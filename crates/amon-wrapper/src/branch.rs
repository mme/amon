//! Which branch the agent's directory is on.
//!
//! Read out of `.git/HEAD` rather than by running `git`. The wrapper spawns no
//! subprocesses at all — every other fact it reports comes from a syscall or a
//! socket — and putting one in the startup path would mean a slow or hung `git`
//! sitting between the user and their agent starting. HEAD is one small file,
//! written by git atomically, and its first line is the whole answer.
//!
//! What that costs: `git` understands submodules, `.git` files pointing at
//! unusual layouts, and repositories configured in ways this does not. Those
//! read as "no branch" here, which is the same as any other directory without
//! one — a blank column rather than a wrong one.

use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::observer::Signal;

/// The daemon's configuration watcher polls at this interval for the same kind
/// of reason. One number for "watch a file a human edits".
const POLL_INTERVAL: Duration = Duration::from_secs(1);

/// Where HEAD lives for `directory`, or `None` if it is not in a repository.
///
/// Walks up looking for `.git`. A directory is the ordinary checkout; a *file*
/// is a linked worktree, whose contents are `gitdir: <path>` pointing at the
/// real per-worktree directory — which has its own HEAD, and is the whole
/// reason this is worth doing: a worktree's path names neither its project nor
/// its branch.
pub fn head_path(directory: &Path) -> Option<PathBuf> {
    for ancestor in directory.ancestors() {
        let candidate = ancestor.join(".git");
        let Ok(metadata) = std::fs::symlink_metadata(&candidate) else {
            continue;
        };
        if metadata.is_dir() {
            return Some(candidate.join("HEAD"));
        }
        // A `.git` file. Its target may be relative to the directory holding it.
        let contents = std::fs::read_to_string(&candidate).ok()?;
        let target = contents.strip_prefix("gitdir:")?.trim();
        let gitdir = PathBuf::from(target);
        let gitdir = if gitdir.is_absolute() {
            gitdir
        } else {
            ancestor.join(gitdir)
        };
        return Some(gitdir.join("HEAD"));
    }
    None
}

/// The branch named in a HEAD file's contents, if it names one.
///
/// `ref: refs/heads/<name>` is a branch. A bare object id is a detached HEAD —
/// mid-rebase, on a tag, bisecting — which has a commit but no branch, and is
/// reported as nothing rather than as a hash. A column that sometimes holds a
/// name and sometimes forty hex digits is harder to read than one that is
/// occasionally blank.
pub fn parse(head: &str) -> Option<String> {
    let name = head.lines().next()?.trim().strip_prefix("ref:")?.trim();
    let name = name.strip_prefix("refs/heads/")?;
    (!name.is_empty()).then(|| name.to_string())
}

/// The branch `directory` is on, if any.
pub fn read(directory: &Path) -> Option<String> {
    let head = head_path(directory)?;
    parse(&std::fs::read_to_string(head).ok()?)
}

/// Follows the branch for as long as the agent runs.
///
/// Polls HEAD's modification time and only re-reads the file when it moves —
/// the same shape as the daemon's configuration watcher, and at the same
/// interval, because these are the same kind of fact: a file a human changes,
/// with nothing waiting on the answer.
///
/// Polling rather than watching, deliberately. Git writes HEAD by creating a
/// temporary file and renaming it over the old one, so an inotify watch on the
/// file itself stops seeing changes after the first — the watch follows the
/// replaced inode. Watching correctly means watching the *directory*, and a
/// worktree's HEAD lives somewhere else again. One `stat` a second is cheaper
/// than being subtly wrong.
pub fn watch(directory: PathBuf, signals: std::sync::mpsc::Sender<Signal>) {
    std::thread::spawn(move || {
        let mut head = head_path(&directory);
        let mut stamp = head.as_ref().and_then(mtime);
        let mut reported = read(&directory);
        loop {
            std::thread::sleep(POLL_INTERVAL);
            // Only re-resolve when there is nothing to watch: a directory that
            // was not a repository may become one, but a repository does not
            // move out from under a running agent.
            if head.is_none() {
                head = head_path(&directory);
            }
            let now = head.as_ref().and_then(mtime);
            if now == stamp {
                continue;
            }
            stamp = now;
            let current = head
                .as_ref()
                .and_then(|path| std::fs::read_to_string(path).ok())
                .and_then(|contents| parse(&contents));
            if current == reported {
                continue;
            }
            reported = current.clone();
            if signals.send(Signal::Branch(current)).is_err() {
                return;
            }
        }
    });
}

fn mtime(path: &PathBuf) -> Option<std::time::SystemTime> {
    std::fs::metadata(path).ok()?.modified().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    /// A directory of its own, outside any repository. Nothing here creates a
    /// real repository: these test what amon reads, and amon reads files.
    fn scratch(name: &str) -> PathBuf {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let root = std::env::temp_dir().join(format!(
            "amon-branch-{}-{}-{}",
            name,
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("scratch dir");
        root
    }

    fn write(path: &Path, contents: &str) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("parent");
        }
        std::fs::write(path, contents).expect("write");
    }

    #[test]
    fn a_branch_head_names_its_branch() {
        assert_eq!(parse("ref: refs/heads/main\n").as_deref(), Some("main"));
        // Slashes are ordinary in branch names and only the first two segments
        // are the prefix.
        assert_eq!(
            parse("ref: refs/heads/fix/detect-flake\n").as_deref(),
            Some("fix/detect-flake")
        );
    }

    #[test]
    fn a_detached_head_names_nothing() {
        // Mid-rebase, on a tag, bisecting: a commit but no branch. Reported as
        // absent rather than as forty hex digits, because a column that
        // sometimes holds a name and sometimes a hash is harder to read than
        // one that is occasionally blank.
        assert_eq!(parse("a2be15dcafe0000000000000000000000000000\n"), None);
        // A tag or a remote ref is not a branch either.
        assert_eq!(parse("ref: refs/tags/v1.0\n"), None);
        assert_eq!(parse(""), None);
    }

    #[test]
    fn an_ordinary_checkout_is_read_from_its_own_git_directory() {
        let root = scratch("plain");
        write(&root.join(".git/HEAD"), "ref: refs/heads/main\n");
        assert_eq!(read(&root).as_deref(), Some("main"));
    }

    #[test]
    fn a_subdirectory_finds_the_repository_above_it() {
        // An agent started in a subdirectory is the normal case, not the
        // exception — `cwd` is wherever the user was standing.
        let root = scratch("deep");
        write(&root.join(".git/HEAD"), "ref: refs/heads/main\n");
        let deep = root.join("crates/amon-wrapper/src");
        std::fs::create_dir_all(&deep).expect("deep dir");
        assert_eq!(read(&deep).as_deref(), Some("main"));
    }

    /// The case the whole feature exists for: a worktree's path names neither
    /// its project nor its branch, and its `.git` is a file pointing at a
    /// per-worktree directory with its own HEAD.
    #[test]
    fn a_linked_worktree_is_read_from_the_directory_its_git_file_points_at() {
        let root = scratch("worktree");
        let repo = root.join("project");
        let tree = root.join("elsewhere/amon-website");
        write(&repo.join(".git/HEAD"), "ref: refs/heads/main\n");
        write(
            &repo.join(".git/worktrees/amon-website/HEAD"),
            "ref: refs/heads/website\n",
        );
        write(
            &tree.join(".git"),
            &format!(
                "gitdir: {}\n",
                repo.join(".git/worktrees/amon-website").display()
            ),
        );
        // The worktree's own branch, not the checkout's.
        assert_eq!(read(&tree).as_deref(), Some("website"));
        assert_eq!(read(&repo).as_deref(), Some("main"));
    }

    #[test]
    fn a_relative_gitdir_is_resolved_against_the_file_that_names_it() {
        // git writes an absolute path today, but the format permits a relative
        // one, and resolving it against the process's cwd would be wrong.
        let root = scratch("relative");
        write(&root.join("real/HEAD"), "ref: refs/heads/side\n");
        write(&root.join("tree/.git"), "gitdir: ../real\n");
        assert_eq!(read(&root.join("tree")).as_deref(), Some("side"));
    }

    #[test]
    fn a_directory_outside_a_repository_has_no_branch() {
        assert_eq!(read(&scratch("bare")), None);
    }
}
