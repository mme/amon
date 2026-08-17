//! Aliases — amon's own, beside the vendored installer.
//!
//! An agent Integration teaches an agent to report to amon. An Alias removes
//! the need to remember to ask for it: `alias claude='amon claude'`, so that
//! running the agent by its own name runs it under the Wrapper.
//!
//! An alias rather than a script on `PATH`, which was the alternative: it is a
//! few lines instead of a generated program, and it cannot recurse, because
//! `PATH` lookup never consults aliases — the `claude` inside the alias is the
//! real one. What it costs is reach. Bash expands aliases only in interactive
//! shells, so an agent started by `foot -e claude`, an editor, or a script runs
//! unwrapped, exactly as it would with no alias at all (ADR-0009).
//!
//! amon does not own the file it edits. `~/.bashrc` is the user's, so amon
//! writes one fenced block, rewrites only what is between the fences, and
//! refuses a `~/.bashrc` that is a symlink — following one would edit a file in
//! somebody's dotfiles repository.

use std::io;
use std::path::{Path, PathBuf};

use crate::api::schema::IntegrationTarget;

/// Fences for the block in the user's shell config. The shape is conda's,
/// deliberately: people already read `>>> name >>>` as a machine-managed region
/// whose insides are not theirs to edit.
const BEGIN: &str = "# >>> amon >>>";
const END: &str = "# <<< amon <<<";

const PREFACE: &str = "\
# Added by amon so that running an agent by its own name runs it under amon.
# `amon remove <agent>` removes its line; deleting this block undoes the lot.
# amon rewrites what is between these two markers and nothing else in this file.";

fn home() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .filter(|home| !home.is_empty())
        .map(PathBuf::from)
}

/// The shell config amon writes into.
///
/// Bash only, because it is Omarchy's default and the only shell installed
/// there. A line written for a shell the user does not run would be worse than
/// saying plainly that amon wrote none.
pub fn shell_config() -> Option<PathBuf> {
    home().map(|home| home.join(".bashrc"))
}

fn missing_home() -> io::Error {
    io::Error::other("cannot find the home directory: HOME is not set")
}

fn alias_line(command: &str) -> String {
    format!("alias {command}='amon {command}'")
}

/// Whether a path is a symlink, which amon will not write through.
fn is_symlink(path: &Path) -> bool {
    std::fs::symlink_metadata(path)
        .map(|metadata| metadata.file_type().is_symlink())
        .unwrap_or(false)
}

/// The alias lines currently inside the block, in the order they appear.
fn aliases_in(existing: &str) -> Vec<String> {
    let lines: Vec<&str> = existing.lines().collect();
    let Some(begin) = lines.iter().position(|line| line.trim() == BEGIN) else {
        return Vec::new();
    };
    let Some(end) = lines.iter().position(|line| line.trim() == END) else {
        return Vec::new();
    };
    if end < begin {
        return Vec::new();
    }
    lines[begin + 1..end]
        .iter()
        .map(|line| line.trim())
        .filter(|line| line.starts_with("alias "))
        .map(str::to_string)
        .collect()
}

fn block(aliases: &[String]) -> String {
    let mut out = String::from(BEGIN);
    out.push('\n');
    out.push_str(PREFACE);
    for alias in aliases {
        out.push('\n');
        out.push_str(alias);
    }
    out.push('\n');
    out.push_str(END);
    out
}

/// Replaces the fenced block, or appends one when it is not there yet.
fn with_block(existing: &str, block: &str) -> String {
    let lines: Vec<&str> = existing.lines().collect();
    let begin = lines.iter().position(|line| line.trim() == BEGIN);
    let end = lines.iter().position(|line| line.trim() == END);

    if let (Some(begin), Some(end)) = (begin, end) {
        if end > begin {
            let mut out = String::new();
            for line in &lines[..begin] {
                out.push_str(line);
                out.push('\n');
            }
            out.push_str(block);
            out.push('\n');
            for line in &lines[end + 1..] {
                out.push_str(line);
                out.push('\n');
            }
            return out;
        }
    }

    let mut out = existing.to_string();
    if !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
    if !out.is_empty() {
        out.push('\n');
    }
    out.push_str(block);
    out.push('\n');
    out
}

/// Removes the fenced block, leaving the rest of the file as it was.
fn without_block(existing: &str) -> Option<String> {
    let lines: Vec<&str> = existing.lines().collect();
    let begin = lines.iter().position(|line| line.trim() == BEGIN)?;
    let end = lines.iter().position(|line| line.trim() == END)?;
    if end < begin {
        return None;
    }

    let mut out = String::new();
    for line in &lines[..begin] {
        out.push_str(line);
        out.push('\n');
    }
    for line in &lines[end + 1..] {
        out.push_str(line);
        out.push('\n');
    }
    // The block is written with a blank line before it; take that back too, so
    // installing and uninstalling repeatedly does not grow the file.
    while out.ends_with("\n\n") {
        out.pop();
    }
    Some(out)
}

fn manual_note(rc: &Path, aliases: &[String]) -> Vec<String> {
    let mut notes = vec![format!(
        "{} is a symlink, so amon has not edited it — add these yourself:",
        rc.display()
    )];
    notes.extend(aliases.iter().map(|alias| format!("  {alias}")));
    notes
}

/// The alias lines currently installed, in block order. Empty when there is no
/// block, no shell config, or a symlinked one amon would not have written.
pub fn installed() -> Vec<String> {
    let Some(rc) = shell_config() else {
        return Vec::new();
    };
    if is_symlink(&rc) {
        return Vec::new();
    }
    let Ok(existing) = std::fs::read_to_string(&rc) else {
        return Vec::new();
    };
    aliases_in(&existing)
}

/// Whether every alias [`install`] would write for this agent is actually in
/// the shell config. [`install`] returning `Ok` is not that: a symlinked
/// `~/.bashrc` makes it print manual instructions instead of writing anything.
pub fn is_installed(target: IntegrationTarget) -> bool {
    let have = installed();
    crate::command_names(target)
        .iter()
        .all(|command| have.contains(&alias_line(command)))
}

/// A note when aliases sit in a shell config amon cannot edit — a symlinked
/// rc whose block was pasted in by hand per [`install`]'s instructions.
/// `None` when nothing is stranded. Callers use this to avoid claiming
/// "unaliased" about lines that are still there.
pub fn stranded() -> Option<String> {
    let rc = shell_config()?;
    if !is_symlink(&rc) {
        return None;
    }
    let existing = std::fs::read_to_string(&rc).ok()?;
    (!aliases_in(&existing).is_empty()).then(|| {
        format!(
            "aliases remain in {} (a symlink amon does not edit) — remove the amon block yourself",
            rc.display()
        )
    })
}

/// Removes the whole fenced block, whichever agents' aliases are in it — the
/// all-or-nothing counterpart of [`uninstall`], for `amon remove` with no
/// target. Returns the notes to print; empty when there was nothing to do.
pub fn remove_all() -> io::Result<Vec<String>> {
    let Some(rc) = shell_config() else {
        return Ok(Vec::new());
    };
    let Ok(existing) = std::fs::read_to_string(&rc) else {
        return Ok(Vec::new());
    };
    let Some(updated) = without_block(&existing) else {
        return Ok(Vec::new());
    };
    if is_symlink(&rc) {
        // The block in a symlinked rc was pasted in by hand (amon refuses to
        // write through links), so taking it out is by hand too.
        return Ok(vec![format!(
            "{} is a symlink, so amon has not edited it — remove the amon block yourself",
            rc.display()
        )]);
    }
    std::fs::write(&rc, updated)?;
    Ok(vec![format!(
        "removed the alias block from {}",
        rc.display()
    )])
}

/// Aliases every command the agent answers to, so that its own name runs it
/// under amon.
///
/// Idempotent: the block holds one line per command however many times it is
/// installed, and a second agent joins that block rather than starting another.
pub fn install(target: IntegrationTarget) -> io::Result<Vec<String>> {
    let rc = shell_config().ok_or_else(missing_home)?;
    let wanted: Vec<String> = crate::command_names(target)
        .iter()
        .map(|command| alias_line(command))
        .collect();

    if is_symlink(&rc) {
        return Ok(manual_note(&rc, &wanted));
    }

    let existing = std::fs::read_to_string(&rc).unwrap_or_default();
    let mut aliases = aliases_in(&existing);
    for alias in &wanted {
        if !aliases.contains(alias) {
            aliases.push(alias.clone());
        }
    }
    aliases.sort();

    let updated = with_block(&existing, &block(&aliases));
    if updated != existing {
        if let Some(parent) = rc.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&rc, updated)?;
    }

    let mut notes = vec![format!("aliased in {}:", rc.display())];
    notes.extend(wanted.iter().map(|alias| format!("  {alias}")));
    notes.push("open a new terminal, or `source ~/.bashrc`, to pick it up".to_string());
    Ok(notes)
}

/// Removes exactly the aliases [`install`] added for this agent, and the block
/// itself once the last one has gone.
pub fn uninstall(target: IntegrationTarget) -> io::Result<Vec<String>> {
    let Some(rc) = shell_config() else {
        return Ok(Vec::new());
    };
    if is_symlink(&rc) {
        return Ok(Vec::new());
    }
    let Ok(existing) = std::fs::read_to_string(&rc) else {
        return Ok(Vec::new());
    };

    let unwanted: Vec<String> = crate::command_names(target)
        .iter()
        .map(|command| alias_line(command))
        .collect();
    let before = aliases_in(&existing);
    let after: Vec<String> = before
        .iter()
        .filter(|alias| !unwanted.contains(alias))
        .cloned()
        .collect();
    if after.len() == before.len() {
        return Ok(Vec::new());
    }

    let updated = if after.is_empty() {
        // Nothing left to say, so the block goes rather than lingering empty.
        without_block(&existing).unwrap_or(existing)
    } else {
        with_block(&existing, &block(&after))
    };
    std::fs::write(&rc, updated)?;

    let mut notes = vec![format!("removed from {}:", rc.display())];
    notes.extend(
        before
            .iter()
            .filter(|alias| unwanted.contains(alias))
            .map(|alias| format!("  {alias}")),
    );
    Ok(notes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_second_agent_joins_the_same_block() {
        let one = with_block("", &block(&[alias_line("claude")]));
        let both = with_block(&one, &block(&[alias_line("claude"), alias_line("codex")]));

        assert_eq!(both.matches(BEGIN).count(), 1, "one block, not two");
        assert!(both.contains("alias claude='amon claude'"));
        assert!(both.contains("alias codex='amon codex'"));
    }

    #[test]
    fn existing_lines_are_read_back_out_of_the_block() {
        let file = with_block("mine\n", &block(&[alias_line("claude")]));
        assert_eq!(aliases_in(&file), vec!["alias claude='amon claude'"]);
    }

    #[test]
    fn adding_and_removing_leaves_the_file_as_it_was() {
        let original = "# theirs\nexport EDITOR=hx\n";
        let added = with_block(original, &block(&[alias_line("claude")]));

        let removed = without_block(&added).expect("the block is there to remove");

        assert_eq!(
            removed, original,
            "uninstalling gives back the file that was there"
        );
    }

    #[test]
    fn a_file_without_a_block_has_no_aliases_and_nothing_to_remove() {
        assert!(aliases_in("export EDITOR=hx\n").is_empty());
        assert!(without_block("export EDITOR=hx\n").is_none());
    }

    #[test]
    fn the_alias_names_the_agent_on_both_sides() {
        // `PATH` lookup never consults aliases, so the `claude` inside is the
        // real one. That is why an alias needs no loop guard at all.
        assert_eq!(alias_line("claude"), "alias claude='amon claude'");
    }
}
