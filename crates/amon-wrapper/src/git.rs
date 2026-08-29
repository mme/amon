//! Where an agent is working: which Project it is in, which branch that is
//! on, and — because an agent can walk away from where it started — which
//! directory it is actually in now.
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

/// How git is laid out around a directory.
///
/// Two roots, because a linked worktree has two. `checkout` is the directory
/// holding `.git` — the worktree itself, or the repository for an ordinary
/// checkout — and is what a path is shown relative to. `project` is the
/// repository that checkout belongs to, which for a worktree is the one it was
/// cut from: a worktree's path names neither its project nor its branch, so
/// the name has to come from where its `.git` file points.
struct Layout {
    checkout: PathBuf,
    project: PathBuf,
    head: PathBuf,
}

/// Walks up looking for `.git`. A directory is the ordinary checkout; a *file*
/// is a linked worktree, whose contents are `gitdir: <path>` pointing at the
/// real per-worktree directory — which has its own HEAD.
fn locate(directory: &Path) -> Option<Layout> {
    for ancestor in directory.ancestors() {
        let candidate = ancestor.join(".git");
        let Ok(metadata) = std::fs::symlink_metadata(&candidate) else {
            continue;
        };
        if metadata.is_dir() {
            return Some(Layout {
                checkout: ancestor.to_path_buf(),
                project: ancestor.to_path_buf(),
                head: candidate.join("HEAD"),
            });
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
        return Some(Layout {
            checkout: ancestor.to_path_buf(),
            project: repository_of(&gitdir).unwrap_or_else(|| ancestor.to_path_buf()),
            head: gitdir.join("HEAD"),
        });
    }
    None
}

/// The repository a per-worktree git directory belongs to.
///
/// git writes `<repository>/.git/worktrees/<name>`, so the repository is three
/// levels up — but only when the layout is exactly that. Anything else (a
/// submodule, a `.git` file pointing somewhere unusual, a future layout) is
/// reported as unknown rather than guessed at, and the caller falls back to
/// the worktree's own directory. A name that is occasionally the worktree's is
/// better than one that is confidently wrong.
fn repository_of(gitdir: &Path) -> Option<PathBuf> {
    let worktrees = gitdir.parent()?;
    if worktrees.file_name()? != "worktrees" {
        return None;
    }
    let git = worktrees.parent()?;
    if git.file_name()? != ".git" {
        return None;
    }
    Some(git.parent()?.to_path_buf())
}

/// Where HEAD lives for `directory`, or `None` if it is not in a repository.
pub fn head_path(directory: &Path) -> Option<PathBuf> {
    locate(directory).map(|location| location.head)
}

/// The Project a directory belongs to, and where inside its checkout it sits.
///
/// `name` is what a row leads with; `subpath` qualifies it and is `None` at
/// the checkout root, which is the ordinary case. Outside a repository there
/// is no Project at all and the directory itself stands in — that is the
/// caller's job, not this function's, because only the caller knows how it
/// wants to render a bare path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Project {
    pub name: String,
    pub subpath: Option<String>,
}

pub fn project(directory: &Path) -> Option<Project> {
    let location = locate(directory)?;
    let name = location.project.file_name()?.to_string_lossy().into_owned();
    let subpath = directory
        .strip_prefix(&location.checkout)
        .ok()
        .map(|rest| rest.to_string_lossy().into_owned())
        .filter(|rest| !rest.is_empty());
    Some(Project { name, subpath })
}

/// Everything a row says about where an agent is working.
///
/// Resolved together because they move together: an agent that walks into a
/// worktree changes its directory, its Project and its branch in one step, and
/// reporting them separately would let a row show a branch from one checkout
/// beside a Project from another.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Location {
    pub cwd: PathBuf,
    pub project: Option<String>,
    pub subpath: Option<String>,
    pub branch: Option<String>,
}

/// What `directory` says about itself, right now.
pub fn location(directory: &Path) -> Location {
    let project = project(directory);
    Location {
        cwd: directory.to_path_buf(),
        project: project.as_ref().map(|project| project.name.clone()),
        subpath: project.and_then(|project| project.subpath),
        branch: read(directory),
    }
}

/// Where the agent process is now, or `None` where that cannot be read.
///
/// The agent's own working directory, not the wrapper's. An agent that moves —
/// into a worktree, into a sibling checkout — takes its Project and its branch
/// with it, and a row that went on naming where it was started would be
/// confidently wrong about both. The kernel already tracks this, so amon reads
/// it rather than asking the agent to announce it: one readlink, and it works
/// for every Agent Kind because it is a fact about a process rather than about
/// an agent.
#[cfg(target_os = "linux")]
pub fn current_directory(pid: u32) -> Option<PathBuf> {
    std::fs::read_link(format!("/proc/{pid}/cwd")).ok()
}

/// Elsewhere the agent is taken to stay where it started, which is what amon
/// did everywhere before this existed.
#[cfg(not(target_os = "linux"))]
pub fn current_directory(_pid: u32) -> Option<PathBuf> {
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

/// Follows where the agent is working for as long as it runs.
///
/// Two things can move: the agent can change directory, and HEAD can move
/// under a directory that stayed put. One poll covers both, because it simply
/// resolves the answer and compares it to the answer it last reported.
///
/// There is no attempt to detect change more cheaply than that. An earlier
/// version watched HEAD's modification time to avoid re-reading it, and that
/// cost more than it saved: a cached path goes stale when a repository is
/// removed under the agent, and two different HEAD files can share a
/// timestamp. Resolving costs a short walk up the ancestors and one small
/// file read, once a second, per agent — less than the reasoning needed to
/// skip it safely.
///
/// Polling rather than watching, deliberately. Git writes HEAD by creating a
/// temporary file and renaming it over the old one, so an inotify watch on the
/// file itself stops seeing changes after the first — the watch follows the
/// replaced inode. Watching correctly means watching the *directory*, and a
/// worktree's HEAD lives somewhere else again.
pub fn watch(pid: u32, start: Location, signals: std::sync::mpsc::Sender<Signal>) {
    std::thread::spawn(move || {
        let mut here = start.cwd.clone();
        // The Location the agent was registered with, not a fresh reading of
        // it. Sampling again here would let a change that landed between
        // registration and this thread starting count as already reported,
        // leaving the daemon holding the older one until something else moved.
        let mut reported = start;
        loop {
            std::thread::sleep(POLL_INTERVAL);

            if let Some(now) = current_directory(pid) {
                here = now;
            }
            let current = location(&here);
            if current == reported {
                continue;
            }
            reported = current.clone();
            if signals.send(Signal::Location(current)).is_err() {
                return;
            }
        }
    });
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

    #[test]
    fn a_checkout_is_its_own_project_and_has_no_subpath() {
        let root = scratch("project-plain");
        write(&root.join(".git/HEAD"), "ref: refs/heads/main\n");
        let project = project(&root).expect("project");
        assert_eq!(project.name, root.file_name().unwrap().to_string_lossy());
        assert_eq!(project.subpath, None);
    }

    #[test]
    fn a_subdirectory_qualifies_its_project() {
        let root = scratch("project-deep");
        write(&root.join(".git/HEAD"), "ref: refs/heads/main\n");
        let deep = root.join("crates/amon-wrapper");
        std::fs::create_dir_all(&deep).expect("deep dir");
        let project = project(&deep).expect("project");
        assert_eq!(project.name, root.file_name().unwrap().to_string_lossy());
        assert_eq!(project.subpath.as_deref(), Some("crates/amon-wrapper"));
    }

    /// The case the Project exists for: a worktree belongs to the repository
    /// it was cut from, and saying so is what lets its rows read alongside
    /// that repository's rather than under an invented name.
    #[test]
    fn a_worktree_belongs_to_the_repository_it_was_cut_from() {
        let root = scratch("project-worktree");
        let repo = root.join("amon.sh");
        let tree = root.join("elsewhere/amon-dictation");
        write(&repo.join(".git/HEAD"), "ref: refs/heads/main\n");
        write(
            &repo.join(".git/worktrees/amon-dictation/HEAD"),
            "ref: refs/heads/dictation\n",
        );
        write(
            &tree.join(".git"),
            &format!(
                "gitdir: {}\n",
                repo.join(".git/worktrees/amon-dictation").display()
            ),
        );
        let project = project(&tree).expect("project");
        assert_eq!(project.name, "amon.sh");
        assert_eq!(project.subpath, None);
        // The branch still comes from the worktree, not the checkout.
        assert_eq!(read(&tree).as_deref(), Some("dictation"));
    }

    #[test]
    fn a_subdirectory_of_a_worktree_is_relative_to_the_worktree() {
        let root = scratch("project-worktree-deep");
        let repo = root.join("amon.sh");
        let tree = root.join("elsewhere/amon-dictation");
        write(&repo.join(".git/HEAD"), "ref: refs/heads/main\n");
        write(
            &repo.join(".git/worktrees/amon-dictation/HEAD"),
            "ref: refs/heads/dictation\n",
        );
        write(
            &tree.join(".git"),
            &format!(
                "gitdir: {}\n",
                repo.join(".git/worktrees/amon-dictation").display()
            ),
        );
        let deep = tree.join("crates/amon-term");
        std::fs::create_dir_all(&deep).expect("deep dir");
        let project = project(&deep).expect("project");
        assert_eq!(project.name, "amon.sh");
        // Relative to the worktree it is in, not to the repository that owns
        // it — the repository does not contain this path at all.
        assert_eq!(project.subpath.as_deref(), Some("crates/amon-term"));
    }

    /// A `.git` file pointing somewhere that is not `<repo>/.git/worktrees/<n>`
    /// — a submodule, or a layout git has not written before. Reported as the
    /// directory's own name rather than as a guess three levels up.
    #[test]
    fn an_unrecognised_gitdir_layout_falls_back_to_its_own_directory() {
        let root = scratch("project-odd");
        write(&root.join("real/HEAD"), "ref: refs/heads/side\n");
        let tree = root.join("tree");
        write(&tree.join(".git"), "gitdir: ../real\n");
        let project = project(&tree).expect("project");
        assert_eq!(project.name, "tree");
    }

    /// Gated for the same reason the function is: elsewhere there is no
    /// `/proc` to read, `current_directory` says so by returning nothing, and
    /// a test that insisted otherwise would fail for being right.
    #[cfg(target_os = "linux")]
    #[test]
    fn the_current_directory_of_a_process_is_its_own() {
        // The whole feature rests on this: an agent that changes directory
        // does so as a process, and the kernel says where it went. Read
        // against this test's own pid, whose cwd is known.
        let pid = std::process::id();
        let here = current_directory(pid).expect("linux reports its own cwd");
        assert_eq!(
            here.canonicalize().ok(),
            std::env::current_dir()
                .ok()
                .and_then(|p| p.canonicalize().ok())
        );
    }

    /// Where there is no `/proc`, an agent is taken to stay where it started
    /// — which is what amon did everywhere before this existed, so the row is
    /// no worse than it was rather than wrong in a new way.
    #[cfg(not(target_os = "linux"))]
    #[test]
    fn without_proc_an_agent_is_taken_to_stay_put() {
        assert_eq!(current_directory(std::process::id()), None);
    }

    #[test]
    fn a_location_resolves_project_and_branch_together() {
        // They move together, so they are read together: a row showing a
        // branch from one checkout beside a Project from another is wrong
        // about both.
        let root = scratch("location");
        write(&root.join(".git/HEAD"), "ref: refs/heads/main\n");
        let deep = root.join("crates/amon-term");
        std::fs::create_dir_all(&deep).expect("deep dir");
        let location = location(&deep);
        assert_eq!(location.cwd, deep);
        assert_eq!(
            location.project.as_deref(),
            root.file_name().unwrap().to_str()
        );
        assert_eq!(location.subpath.as_deref(), Some("crates/amon-term"));
        assert_eq!(location.branch.as_deref(), Some("main"));
    }

    #[test]
    fn walking_out_of_a_repository_clears_what_it_said() {
        // Absent has to be expressible, not just a different value: an agent
        // that leaves a repository must stop naming the last one it was in.
        let root = scratch("location-out");
        write(&root.join(".git/HEAD"), "ref: refs/heads/main\n");
        let outside = scratch("location-bare");
        let location = location(&outside);
        assert_eq!(location.project, None);
        assert_eq!(location.branch, None);
    }

    #[test]
    fn a_directory_outside_a_repository_has_no_project() {
        assert_eq!(project(&scratch("project-bare")), None);
    }
}
