//! opencode's activity plugin (ADR-0021: a seam).
//!
//! opencode loads any `.js` in its `plugins/` directory, so installing is one
//! amon-owned file beside herdr's and uninstalling is its removal. The plugin
//! reports the submitted prompt (the `chat.message` hook), each tool call
//! (`tool.execute.before`) and the reply's first line (`message.part.updated`)
//! as Activity — opencode's screen amon has never read, so its plugin API is
//! the only Narration it has.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use super::{state_of, HookState};
use crate::integration::opencode_dir;

const INSTALL_NAME: &str = "amon-activity.js";
const ASSET: &str = include_str!("../assets/opencode-activity.js");
/// Mirrors the `AMON_OPENCODE_ACTIVITY_VERSION` stamp in the extension.
pub const VERSION: u32 = 1;

fn plugins_dir() -> io::Result<PathBuf> {
    Ok(opencode_dir()?.join("plugins"))
}

fn extension_path() -> io::Result<PathBuf> {
    Ok(plugins_dir()?.join(INSTALL_NAME))
}

/// Writes the extension. Idempotent. `None` when opencode is not set up here.
pub fn install() -> io::Result<Option<String>> {
    let Ok(dir) = opencode_dir() else {
        return Ok(None);
    };
    if !dir.is_dir() {
        return Ok(None);
    }
    write_extension(&plugins_dir()?)?;
    Ok(Some(
        "activity plugin — reports the prompt, each tool call, and the reply".to_string(),
    ))
}

pub(crate) fn write_extension(dir: &Path) -> io::Result<()> {
    fs::create_dir_all(dir)?;
    fs::write(dir.join(INSTALL_NAME), ASSET)
}

/// Removes the extension. Safe when nothing is installed.
pub fn uninstall() -> io::Result<()> {
    let path = extension_path()?;
    if path.exists() {
        fs::remove_file(&path)?;
    }
    Ok(())
}

pub fn state() -> io::Result<HookState> {
    state_of(
        &extension_path()?,
        &format!("AMON_OPENCODE_ACTIVITY_VERSION={VERSION}"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_asset_stamps_the_version_state_checks_for() {
        assert!(
            ASSET.contains(&format!("AMON_OPENCODE_ACTIVITY_VERSION={VERSION}")),
            "the extension and state() disagree on the version stamp"
        );
    }

    #[test]
    fn the_extension_reports_and_never_edits() {
        // The extension is content-only: it must speak the activity method and
        // stay out of state and session reporting, which remain herdr's.
        assert!(ASSET.contains("agent.report_activity"));
        assert!(
            !ASSET.contains("agent.report_state"),
            "state is herdr's job"
        );
        for hook in [
            "chat.message",
            "tool.execute.before",
            "message.part.updated",
        ] {
            assert!(ASSET.contains(hook), "missing subscription: {hook}");
        }
    }

    #[test]
    fn installing_writes_the_file_and_uninstall_is_removal() {
        let dir = tempfile::tempdir().expect("tempdir");
        let extensions = dir.path().join("plugins");
        write_extension(&extensions).expect("install");
        let path = extensions.join(INSTALL_NAME);
        assert!(path.exists());
        write_extension(&extensions).expect("install again");
        assert_eq!(
            std::fs::read_to_string(&path).expect("read"),
            ASSET,
            "reinstall did not converge on the shipped asset"
        );
    }
}
