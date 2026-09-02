//! amon's own Claude prompt hook (ADR-0021: a seam).
//!
//! herdr registers exactly one Claude hook — `SessionStart`, for session
//! identity — and its migration actively *removes* `UserPromptSubmit`, which
//! it once used as a state signal and gave up when screen detection replaced
//! it. amon wants that event back, not for state but for content: the
//! submitted prompt is a turn's boundary, reported exactly as typed rather
//! than inferred from the screen.
//!
//! This is additive and stays entirely amon's. The script is an amon asset,
//! not a vendored one, so `revendor.sh` never touches it. The registration
//! goes through herdr's own formatting-preserving CST splice primitives —
//! the JSON surgery is the load-bearing part and reimplementing it would only
//! re-earn herdr's bug fixes. And because herdr's install removes this event,
//! amon's step runs *after* it, on every `setup` and `setup --upgrade`, and is
//! idempotent: herdr strips it, amon puts it back.
//!
//! What amon adds, amon removes: `uninstall` takes the script and its
//! registration away, and `doctor` reports its health, so the lifecycle of
//! this one addition is amon's and never orphaned.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use jsonc_parser::cst::CstRootNode;

use crate::integration::{
    append_array_element, append_object_property, claude_dir, hook_command,
    is_matching_command_hook, parse_ast_root_object, strict_parse_options,
};

const INSTALL_NAME: &str = "amon-prompt-state.sh";
const ASSET: &str = include_str!("../assets/claude-prompt-hook.sh");
const EVENT: &str = "UserPromptSubmit";
/// Bumped when the asset changes, so `doctor` can tell a stale copy from a
/// current one. Mirrors the `AMON_PROMPT_HOOK_VERSION` stamp in the script.
pub const VERSION: u32 = 1;

/// Whether the installed prompt hook is absent, stale, or current.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptHookState {
    NotInstalled,
    Outdated,
    Current,
}

fn hook_path() -> io::Result<PathBuf> {
    Ok(claude_dir()?.join("hooks").join(INSTALL_NAME))
}

fn settings_path() -> io::Result<PathBuf> {
    Ok(claude_dir()?.join("settings.json"))
}

/// Installs the script and registers it for `UserPromptSubmit`. Idempotent —
/// safe to run after every `setup`, which is required, since herdr's own
/// install removes this event.
///
/// Returns the note to print, or `None` when Claude is not set up here (no
/// `.claude` directory) — in which case there is nothing to add the hook to
/// and the caller simply skips it.
pub fn install() -> io::Result<Option<String>> {
    let Ok(dir) = claude_dir() else {
        return Ok(None);
    };
    if !dir.exists() {
        return Ok(None);
    }

    let path = hook_path()?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&path, ASSET)?;
    set_executable(&path)?;

    register(&settings_path()?, &path)?;
    Ok(Some(
        "prompt hook — reports what each turn is working on".to_string(),
    ))
}

/// Removes the registration and the script. Safe when nothing is installed.
pub fn uninstall() -> io::Result<()> {
    let path = hook_path()?;
    let settings = settings_path()?;
    if settings.exists() {
        deregister(&settings, &path)?;
    }
    if path.exists() {
        fs::remove_file(&path)?;
    }
    Ok(())
}

/// What `doctor` reports for the prompt hook.
pub fn state() -> io::Result<PromptHookState> {
    let path = hook_path()?;
    if !path.exists() {
        return Ok(PromptHookState::NotInstalled);
    }
    let installed = fs::read_to_string(&path)?;
    let version_line = format!("AMON_PROMPT_HOOK_VERSION={VERSION}");
    if installed.contains(&version_line) {
        Ok(PromptHookState::Current)
    } else {
        Ok(PromptHookState::Outdated)
    }
}

/// The command string the hook is registered under. Removal matches on it
/// exactly, so a hook the user added by hand for the same event is left alone.
fn command(path: &Path) -> String {
    hook_command(path, None)
}

/// One `UserPromptSubmit` entry, as compact JSON. Same shape herdr writes for
/// its own hooks — a matcher and a single command — so both events read alike.
fn entry_json(path: &Path) -> io::Result<String> {
    let command = serde_json::to_string(&command(path))?;
    Ok(format!(
        "{{\"matcher\":\"*\",\"hooks\":[{{\"type\":\"command\",\"command\":{command}}}]}}"
    ))
}

/// Adds the hook to `settings`, preserving the file's formatting — only the
/// bytes that change are touched, because the splice is herdr's own
/// CST-preserving primitive (ADR-0021: a seam). Idempotent: a second call
/// finds the entry already present and writes nothing.
fn register(settings: &Path, hook: &Path) -> io::Result<()> {
    let content = match fs::read_to_string(settings) {
        Ok(text) if !text.trim().is_empty() => text,
        _ => "{}".to_string(),
    };
    // The command carries this hook's unique path, so its presence is proof
    // the entry is already there — no need to walk the tree to add idempotency.
    if content.contains(&command(hook)) {
        return Ok(());
    }
    let entry = entry_json(hook)?;
    let root = parse_ast_root_object(&content, settings)?;
    let updated = match root.get_object("hooks") {
        None => append_object_property(
            &content,
            &root,
            "hooks",
            &format!("{{\"{EVENT}\":[{entry}]}}"),
        ),
        Some(hooks) => match hooks.get_array(EVENT) {
            None => append_object_property(&content, hooks, EVENT, &format!("[{entry}]")),
            Some(array) => append_array_element(&content, array, &entry),
        },
    };
    write(settings, &updated)
}

/// Removes our entry, preserving formatting via jsonc's CST — the same library
/// and technique herdr's own removal uses. Empty structures left behind are
/// pruned so an uninstall leaves no `"UserPromptSubmit": []` husk.
fn deregister(settings: &Path, hook: &Path) -> io::Result<()> {
    let content = fs::read_to_string(settings)?;
    let command = command(hook);
    let root = CstRootNode::parse(&content, &strict_parse_options()).map_err(|err| {
        io::Error::other(format!("failed to parse {}: {err}", settings.display()))
    })?;
    let Some(object) = root.value().and_then(|value| value.as_object()) else {
        return Ok(());
    };
    let Some(hooks) = object
        .get("hooks")
        .and_then(|property| property.object_value())
    else {
        return Ok(());
    };
    let Some(event) = hooks.get(EVENT) else {
        return Ok(());
    };
    let Some(entries) = event.array_value() else {
        return Ok(());
    };

    for entry in entries.elements() {
        let Some(entry_object) = entry.as_object() else {
            continue;
        };
        if let Some(commands) = entry_object.get("hooks").and_then(|p| p.array_value()) {
            for command_entry in commands.elements() {
                let matches = command_entry
                    .to_serde_value()
                    .is_some_and(|value| is_matching_command_hook(&value, &command));
                if matches {
                    command_entry.remove();
                }
            }
            if commands.elements().is_empty() {
                entry.remove();
            }
        }
    }
    if entries.elements().is_empty() {
        event.remove();
    }
    write(settings, &root.to_string())
}

fn write(path: &Path, text: &str) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, text)
}

#[cfg(unix)]
fn set_executable(path: &std::path::Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o755))
}

#[cfg(not(unix))]
fn set_executable(_path: &std::path::Path) -> io::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests;
