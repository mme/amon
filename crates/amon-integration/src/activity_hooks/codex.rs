//! Codex's prompt hook (ADR-0021: a seam).
//!
//! Codex's hook engine mirrors Claude's: JSON registrations in
//! `~/.codex/hooks.json`, delivered to scripts as JSON on stdin with a
//! `hook_event_name`, gated by `[features] hooks = true` — which herdr's own
//! install already ensures, along with the file itself. herdr registers only
//! `SessionStart` there (its migration removes `UserPromptSubmit`, an event it
//! once used for state), so the prompt event is amon's alone, exactly as with
//! Claude.
//!
//! Unlike Claude's `settings.json`, `hooks.json` is machine-owned: herdr
//! rewrites it whole with a pretty-printer on every install. Editing it the
//! same way is therefore fidelity, not a shortcut — the file has no user
//! formatting to preserve. The semantic work — deduplication, matching only
//! our own command on removal — is still herdr's `ensure_command_hook` and
//! `remove_command_hook`.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use super::{set_executable, state_of, HookState};
use crate::integration::{codex_dir, ensure_command_hook, hook_command, remove_command_hook};

const INSTALL_NAME: &str = "amon-prompt-state.sh";
const ASSET: &str = include_str!("../assets/codex-prompt-hook.sh");
const EVENT: &str = "UserPromptSubmit";
/// Mirrors the `AMON_CODEX_PROMPT_HOOK_VERSION` stamp in the script.
pub const VERSION: u32 = 1;

fn hook_path() -> io::Result<PathBuf> {
    Ok(codex_dir()?.join(INSTALL_NAME))
}

fn hooks_json_path() -> io::Result<PathBuf> {
    Ok(codex_dir()?.join("hooks.json"))
}

/// Installs the script and registers it for `UserPromptSubmit`. Idempotent.
/// `None` when Codex is not set up here.
pub fn install() -> io::Result<Option<String>> {
    let Ok(dir) = codex_dir() else {
        return Ok(None);
    };
    if !dir.exists() {
        return Ok(None);
    }
    let path = hook_path()?;
    fs::write(&path, ASSET)?;
    set_executable(&path)?;
    register(&hooks_json_path()?, &path)?;
    Ok(Some(
        // Codex gates newly registered hooks behind a one-time review: on its
        // next launch it asks the user to trust them before they run. That is
        // Codex's security model, not a fault, and amon must not try to steer
        // around it — the note is how the user learns the prompt is expected.
        "prompt hook — reports what each turn is working on (codex asks to trust it on next launch)"
            .to_string(),
    ))
}

/// Removes the registration and the script. Safe when nothing is installed.
pub fn uninstall() -> io::Result<()> {
    let path = hook_path()?;
    let hooks_json = hooks_json_path()?;
    if hooks_json.exists() {
        deregister(&hooks_json, &path)?;
    }
    if path.exists() {
        fs::remove_file(&path)?;
    }
    Ok(())
}

pub fn state() -> io::Result<HookState> {
    state_of(
        &hook_path()?,
        &format!("AMON_CODEX_PROMPT_HOOK_VERSION={VERSION}"),
    )
}

fn register(hooks_json: &Path, hook: &Path) -> io::Result<()> {
    let mut root = read(hooks_json)?;
    let hooks = crate::integration::ensure_hooks_object(
        &mut root,
        hooks_json,
        "codex hooks file",
        "codex hooks file hooks",
    )?;
    // ensure_command_hook is idempotent by the exact command, so a second
    // install adds nothing. Matcher `None`, as herdr registers its own codex
    // hook — codex's engine, unlike Claude's, takes none.
    ensure_command_hook(hooks, EVENT, hook_command(hook, None), 10, None)?;
    write(hooks_json, &root)
}

fn deregister(hooks_json: &Path, hook: &Path) -> io::Result<()> {
    let mut root = read(hooks_json)?;
    if let Some(hooks) = root.get_mut("hooks").and_then(Value::as_object_mut) {
        // Matched by our exact command; a hook the user added by hand for the
        // same event is left alone.
        remove_command_hook(hooks, EVENT, &hook_command(hook, None))?;
        if let Some(entries) = hooks.get(EVENT) {
            if entries.as_array().is_some_and(Vec::is_empty) {
                hooks.remove(EVENT);
            }
        }
    }
    write(hooks_json, &root)
}

fn read(path: &Path) -> io::Result<Value> {
    match fs::read_to_string(path) {
        Ok(text) if !text.trim().is_empty() => serde_json::from_str(&text)
            .map_err(|err| io::Error::other(format!("failed to parse {}: {err}", path.display()))),
        Ok(_) => Ok(json!({})),
        // Absent is the only "treat as empty" case; any other read error must
        // not become an empty object that then overwrites the real file.
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(json!({})),
        Err(error) => Err(error),
    }
}

fn write(path: &Path, value: &Value) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    // Pretty-printed whole-file write is this file's own convention: herdr's
    // codex install writes it the same way.
    let mut text = serde_json::to_string_pretty(value)?;
    text.push('\n');
    fs::write(path, text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_asset_stamps_the_version_state_checks_for() {
        assert!(
            ASSET.contains(&format!("AMON_CODEX_PROMPT_HOOK_VERSION={VERSION}")),
            "the script and state() disagree on the version stamp"
        );
    }

    #[test]
    fn registering_is_idempotent_and_coexists_with_herdrs_hook() {
        let dir = tempfile::tempdir().expect("tempdir");
        let hooks_json = dir.path().join("hooks.json");
        let hook = dir.path().join("amon-prompt-state.sh");
        // herdr's own SessionStart registration is already there.
        std::fs::write(
            &hooks_json,
            r#"{"hooks":{"SessionStart":[{"hooks":[{"type":"command","command":"theirs","timeout":10}]}]}}"#,
        )
        .expect("seed");

        register(&hooks_json, &hook).expect("register");
        register(&hooks_json, &hook).expect("register again");

        let root: Value =
            serde_json::from_str(&std::fs::read_to_string(&hooks_json).expect("read"))
                .expect("json");
        assert_eq!(
            root["hooks"][EVENT].as_array().map(Vec::len),
            Some(1),
            "registered twice: {root:#}"
        );
        assert!(
            root["hooks"]["SessionStart"].is_array(),
            "clobbered herdr's hook: {root:#}"
        );
    }

    #[test]
    fn deregistering_removes_only_ours_and_prunes_the_event() {
        let dir = tempfile::tempdir().expect("tempdir");
        let hooks_json = dir.path().join("hooks.json");
        let hook = dir.path().join("amon-prompt-state.sh");
        std::fs::write(&hooks_json, "{}").expect("seed");

        register(&hooks_json, &hook).expect("register");
        deregister(&hooks_json, &hook).expect("deregister");

        let root: Value =
            serde_json::from_str(&std::fs::read_to_string(&hooks_json).expect("read"))
                .expect("json");
        assert!(
            root["hooks"].get(EVENT).is_none(),
            "empty event husk left behind: {root:#}"
        );
    }

    #[test]
    fn a_read_error_never_becomes_an_empty_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let hooks_json = dir.path().join("hooks.json");
        let hook = dir.path().join("amon-prompt-state.sh");
        std::fs::write(&hooks_json, "not json at all").expect("seed");

        assert!(register(&hooks_json, &hook).is_err(), "should refuse");
        assert_eq!(
            std::fs::read_to_string(&hooks_json).expect("read"),
            "not json at all",
            "the file was altered"
        );
    }
}
