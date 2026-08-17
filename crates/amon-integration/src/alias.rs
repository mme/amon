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

/// The alias lines inside every fenced block, in the order they appear. Every
/// block: a hand-maintained config can end up with more than one (two agents'
/// symlink instructions, pasted verbatim), and bash activates them all, so
/// reading only the first would hide live aliases.
fn aliases_in(existing: &str) -> Vec<String> {
    let mut aliases = Vec::new();
    let mut in_block = false;
    for line in existing.lines() {
        let trimmed = line.trim();
        if trimmed == BEGIN {
            in_block = true;
        } else if trimmed == END {
            in_block = false;
        } else if in_block && trimmed.starts_with("alias ") {
            aliases.push(trimmed.to_string());
        }
    }
    aliases
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

/// Whether a `# >>> amon >>>` fence is missing its closing marker. The block
/// walkers treat everything inside a fence as amon's to rewrite, so a
/// truncated fence would swallow the rest of the user's file — every writer
/// checks this first and refuses to edit rather than destroy.
fn fence_unterminated(existing: &str) -> bool {
    let mut in_block = false;
    for line in existing.lines() {
        let trimmed = line.trim();
        if !in_block && trimmed == BEGIN {
            in_block = true;
        } else if in_block && trimmed == END {
            in_block = false;
        }
    }
    in_block
}

/// The note printed instead of editing a file with a truncated fence.
fn truncated_note(rc: &Path) -> String {
    format!(
        "the amon block in {} is missing its closing `{END}` — repair it by hand before amon edits this file",
        rc.display()
    )
}

/// The lines outside every fenced block, and where the first block began —
/// the shared walk under [`with_block`] and [`without_block`]. Callers have
/// already refused files where [`fence_unterminated`] holds.
fn split_blocks(existing: &str) -> (Vec<&str>, Option<usize>) {
    let mut kept = Vec::new();
    let mut in_block = false;
    let mut first_block = None;
    for line in existing.lines() {
        let trimmed = line.trim();
        if !in_block && trimmed == BEGIN {
            in_block = true;
            first_block.get_or_insert(kept.len());
        } else if in_block {
            if trimmed == END {
                in_block = false;
            }
        } else {
            kept.push(line);
        }
    }
    (kept, first_block)
}

/// Replaces the fenced block — every fenced block, merged into one at the
/// first's position — or appends one when none is there yet. Collapsing
/// duplicates matters because [`aliases_in`] feeds this the union of all
/// blocks' lines; leaving a second block behind would double them.
fn with_block(existing: &str, block: &str) -> String {
    let (kept, first_block) = split_blocks(existing);

    let mut out = String::new();
    let index = first_block.unwrap_or(kept.len());
    for line in &kept[..index] {
        out.push_str(line);
        out.push('\n');
    }
    if first_block.is_none() && !out.is_empty() {
        out.push('\n');
    }
    out.push_str(block);
    out.push('\n');
    for line in &kept[index..] {
        out.push_str(line);
        out.push('\n');
    }
    out
}

/// Removes every fenced block, leaving the rest of the file as it was.
fn without_block(existing: &str) -> Option<String> {
    let (kept, first_block) = split_blocks(existing);
    first_block?;

    let mut out = String::new();
    for line in kept {
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

/// The instructions for a symlinked rc. The full fenced block, not bare alias
/// lines: everything that later *recognizes* hand-pasted aliases —
/// [`present`], [`stranded_for`], `amon doctor` — reads only inside the
/// fences, so instructing fenceless lines would create aliases amon can see
/// working in bash but never account for.
fn manual_note(rc: &Path, aliases: &[String]) -> Vec<String> {
    let mut notes = vec![format!(
        "{} is a symlink, so amon has not edited it — add this block yourself",
        rc.display()
    )];
    notes.push("(or just its alias lines, into an existing amon block):".to_string());
    notes.extend(block(aliases).lines().map(|line| format!("  {line}")));
    notes
}

/// The alias lines in the shell config as bash will read them — through a
/// symlink too. Diagnostics want what is *active*; [`installed`] answers the
/// narrower question of what amon may edit.
pub fn present() -> Vec<String> {
    let Some(rc) = shell_config() else {
        return Vec::new();
    };
    let Ok(existing) = std::fs::read_to_string(&rc) else {
        return Vec::new();
    };
    aliases_in(&existing)
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

/// [`stranded`], narrowed to one agent: the notes for a targeted removal,
/// naming only that agent's lines. Telling someone removing claude to delete
/// the whole block would take codex's still-wanted alias with it.
pub fn stranded_for(target: IntegrationTarget) -> Vec<String> {
    let Some(rc) = shell_config() else {
        return Vec::new();
    };
    if !is_symlink(&rc) {
        return Vec::new();
    }
    let Ok(existing) = std::fs::read_to_string(&rc) else {
        return Vec::new();
    };
    let theirs = aliases_in(&existing);
    let mine: Vec<String> = crate::command_names(target)
        .iter()
        .map(|command| alias_line(command))
        .filter(|wanted| theirs.contains(wanted))
        .collect();
    if mine.is_empty() {
        return Vec::new();
    }
    let mut notes = vec![format!(
        "{} is a symlink amon does not edit — remove these lines yourself:",
        rc.display()
    )];
    notes.extend(mine.iter().map(|line| format!("  {line}")));
    notes
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
    if fence_unterminated(&existing) {
        return Ok(vec![truncated_note(&rc)]);
    }
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
    if fence_unterminated(&existing) {
        return Ok(vec![truncated_note(&rc)]);
    }
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
    if fence_unterminated(&existing) {
        return Ok(vec![truncated_note(&rc)]);
    }

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

    #[test]
    fn a_truncated_fence_freezes_the_file() {
        // Everything inside a fence is amon's to rewrite, so a block missing
        // its closing marker would swallow the rest of the user's file. The
        // writers all check this and refuse; the readers still see the
        // aliases, because bash does too.
        let truncated = format!("{BEGIN}\n{}\nexport THEIRS=1\n", alias_line("claude"));
        assert!(fence_unterminated(&truncated));
        assert!(!fence_unterminated(&block(&[alias_line("claude")])));
        assert_eq!(
            aliases_in(&truncated),
            vec![alias_line("claude")],
            "still-active aliases stay visible"
        );
    }

    #[test]
    fn two_pasted_blocks_are_read_removed_and_merged_as_one() {
        // Two agents' symlink instructions, pasted verbatim, make two blocks.
        // Bash activates both, so every reader must see both — and the first
        // amon-side write must collapse them back to one.
        let pasted = format!(
            "mine\n{}\ntheirs\n{}\n",
            block(&[alias_line("claude")]),
            block(&[alias_line("codex")])
        );

        assert_eq!(
            aliases_in(&pasted),
            vec![alias_line("claude"), alias_line("codex")],
            "aliases from every block are seen"
        );
        assert_eq!(
            without_block(&pasted).expect("blocks found"),
            "mine\ntheirs\n",
            "removal takes every block"
        );
        let merged = with_block(
            &pasted,
            &block(&[alias_line("claude"), alias_line("codex")]),
        );
        assert_eq!(merged.matches(BEGIN).count(), 1, "one block after a write");
        assert!(merged.contains("alias codex='amon codex'"));
        assert!(merged.contains("theirs"), "user content survives");
    }

    #[test]
    fn the_manual_instructions_paste_a_block_the_scanners_recognize() {
        // A user who follows the symlink instructions to the letter must end
        // up visible to everything that reads inside the fences — doctor,
        // stranded_for, present. Bare lines outside a fence would work in
        // bash and be invisible to all of them.
        let notes = manual_note(Path::new("/dot/.bashrc"), &[alias_line("claude")]);
        let pasted: String = notes
            .iter()
            .skip(2)
            .map(|line| format!("{}\n", line.trim_start()))
            .collect();
        assert_eq!(
            aliases_in(&pasted),
            vec![alias_line("claude")],
            "what the note says to paste is what the fence scanners parse"
        );
    }
}
