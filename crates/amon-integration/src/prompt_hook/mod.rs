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

use jsonc_parser::cst::{CstInputValue, CstRootNode};
use jsonc_parser::json;

use crate::integration::{
    claude_dir, hook_command, is_matching_command_hook, strict_parse_options,
};

const INSTALL_NAME: &str = "amon-prompt-state.sh";
const ASSET: &str = include_str!("../assets/claude-prompt-hook.sh");
const EVENT: &str = "UserPromptSubmit";
/// Bumped when the asset changes, so `doctor` can tell a stale copy from a
/// current one. Mirrors the `AMON_PROMPT_HOOK_VERSION` stamp in the script.
pub const VERSION: u32 = 2;

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

/// One `UserPromptSubmit` entry, as a CST input value — the same shape herdr
/// writes for its own hooks (a matcher and a single command), built through
/// jsonc's `json!` so escaping is the library's job, not ours.
fn entry_input(path: &Path) -> CstInputValue {
    let command = command(path);
    json!({
        matcher: "*",
        hooks: [{
            "type": "command",
            command: command,
        }],
    })
}

/// Adds the hook to `settings`, preserving the file's formatting — only the
/// bytes that change are touched, because the edit is herdr's own CST
/// (ADR-0021: a seam). Idempotent: an entry with our command already present
/// leaves the file untouched.
///
/// Robust against a settings file amon did not write. A read error that is not
/// "file absent" propagates rather than being taken as an empty file — the
/// caller is non-fatal, so the hook is skipped and the file is left exactly as
/// it was, never overwritten. A `hooks` value that is not an object, or an
/// event value that is not an array, is refused for the same reason: appending
/// beside it would write a duplicate key and make the file ambiguous.
fn register(settings: &Path, hook: &Path) -> io::Result<()> {
    let content = match fs::read_to_string(settings) {
        Ok(text) => text,
        // Absent is the only "treat as empty" case. Anything else — a
        // permissions or I/O error, invalid UTF-8 — must not become an empty
        // object that then overwrites the real file.
        Err(error) if error.kind() == io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(error),
    };
    let content = if content.trim().is_empty() {
        "{}".to_string()
    } else {
        content
    };

    let root = CstRootNode::parse(&content, &strict_parse_options()).map_err(|err| {
        io::Error::other(format!("failed to parse {}: {err}", settings.display()))
    })?;
    let Some(object) = root.value().and_then(|value| value.as_object()) else {
        return Err(io::Error::other(format!(
            "{} is not a JSON object",
            settings.display()
        )));
    };

    let hooks = match object.get("hooks") {
        None => object
            .append("hooks", CstInputValue::Object(Vec::new()))
            .object_value()
            .ok_or_else(|| io::Error::other("failed to create hooks object"))?,
        Some(property) => property.object_value().ok_or_else(|| {
            io::Error::other(format!(
                "hooks in {} is not an object; leaving it alone",
                settings.display()
            ))
        })?,
    };

    let entries = match hooks.get(EVENT) {
        None => hooks
            .append(EVENT, CstInputValue::Array(Vec::new()))
            .array_value()
            .ok_or_else(|| io::Error::other("failed to create event array"))?,
        Some(property) => property.array_value().ok_or_else(|| {
            io::Error::other(format!(
                "{EVENT} in {} is not an array; leaving it alone",
                settings.display()
            ))
        })?,
    };

    // Idempotent by the parsed command, not a raw substring: an entry whose
    // path needed JSON escaping still matches, and the same command text
    // appearing elsewhere in the file does not.
    let command = command(hook);
    let already = entries.elements().iter().any(|entry| {
        entry
            .as_object()
            .and_then(|object| object.get("hooks"))
            .and_then(|property| property.array_value())
            .is_some_and(|commands| {
                commands.elements().iter().any(|command_entry| {
                    command_entry
                        .to_serde_value()
                        .is_some_and(|value| is_matching_command_hook(&value, &command))
                })
            })
    });
    if already {
        return Ok(());
    }

    entries.append(entry_input(hook));
    write(settings, &root.to_string())
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
