//! grok's prompt hook (ADR-0020: a seam).
//!
//! grok merges every `~/.grok/hooks/*.json` at startup, so a hook is installed
//! by dropping a dedicated amon config file (and its script) into that
//! directory — amon edits nobody else's config, the purest form of a seam.
//! herdr registers only `session_start` there; `user_prompt_submit` is amon's
//! alone. Its narration comes from the screen carrier (grok marks tool calls
//! with ◈); this supplies the prompt cleanly, the render being too cluttered
//! to scrape.

use std::fs;
use std::io;
use std::path::PathBuf;

use serde_json::json;

use super::{set_executable, state_of, HookState};
use crate::integration::{grok_dir, hook_command};

const SCRIPT_NAME: &str = "amon-prompt-state.sh";
const CONFIG_NAME: &str = "amon-activity.json";
const ASSET: &str = include_str!("../assets/grok-prompt-hook.sh");
/// Mirrors the `AMON_GROK_PROMPT_HOOK_VERSION` stamp in the script.
pub const VERSION: u32 = 2;

fn hooks_dir() -> io::Result<PathBuf> {
    Ok(grok_dir()?.join("hooks"))
}

fn script_path() -> io::Result<PathBuf> {
    Ok(hooks_dir()?.join(SCRIPT_NAME))
}

fn config_path() -> io::Result<PathBuf> {
    Ok(hooks_dir()?.join(CONFIG_NAME))
}

/// Writes the script and its dedicated config. Idempotent. `None` when grok is
/// not set up here.
pub fn install() -> io::Result<Option<String>> {
    let Ok(dir) = grok_dir() else {
        return Ok(None);
    };
    if !dir.is_dir() {
        return Ok(None);
    }
    let hooks = hooks_dir()?;
    fs::create_dir_all(&hooks)?;
    let script = script_path()?;
    fs::write(&script, ASSET)?;
    set_executable(&script)?;

    // A dedicated file grok merges alongside the user's own — no shared file to
    // edit, so registration is just writing it.
    let config = json!({
        "hooks": {
            "UserPromptSubmit": [
                { "hooks": [
                    { "type": "command", "command": hook_command(&script, None), "timeout": 10 }
                ] }
            ]
        }
    });
    let mut text = serde_json::to_string_pretty(&config)?;
    text.push('\n');
    fs::write(config_path()?, text)?;
    Ok(Some(
        "prompt hook — reports what each turn is working on".to_string(),
    ))
}

/// Removes the script and config. Safe when nothing is installed.
pub fn uninstall() -> io::Result<()> {
    for path in [script_path()?, config_path()?] {
        if path.exists() {
            fs::remove_file(&path)?;
        }
    }
    Ok(())
}

pub fn state() -> io::Result<HookState> {
    state_of(
        &script_path()?,
        &format!("AMON_GROK_PROMPT_HOOK_VERSION={VERSION}"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_asset_stamps_the_version_state_checks_for() {
        assert!(
            ASSET.contains(&format!("AMON_GROK_PROMPT_HOOK_VERSION={VERSION}")),
            "the script and state() disagree on the version stamp"
        );
    }

    #[test]
    fn the_script_hands_its_json_to_amon() {
        // What it reports, and on which event, is hook_input.rs's to test.
        assert!(ASSET.contains("hook input grok-prompt"));
        assert!(!ASSET.contains("python"), "no interpreter (issue #77)");
    }
}
