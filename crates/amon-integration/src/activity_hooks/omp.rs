//! omp's activity extension (ADR-0021: a seam).
//!
//! omp (oh-my-pi) is a pi fork and loads extensions the same way: the whole
//! install is one amon-owned file beside herdr's, the whole uninstall is its
//! removal. The extension reports the submitted prompt as a turn boundary and
//! tool calls / the reply's first line as narration — omp's screen has never
//! been readable by amon, so these events are the only Narration it has. Its
//! fork predates pi's newest events (no agent_settled, no ui_prompt_*), and
//! the extension subscribes to nothing newer than the fork carries.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use super::{state_of, HookState};
use crate::integration::omp_extension_dir;

const INSTALL_NAME: &str = "amon-activity.ts";
const ASSET: &str = include_str!("../assets/omp-activity.ts");
/// Mirrors the `AMON_OMP_ACTIVITY_VERSION` stamp in the extension.
pub const VERSION: u32 = 1;

fn extension_path() -> io::Result<PathBuf> {
    Ok(omp_extension_dir()?.join(INSTALL_NAME))
}

/// Writes the extension. Idempotent. `None` when omp is not set up here.
pub fn install() -> io::Result<Option<String>> {
    let Ok(dir) = omp_extension_dir() else {
        return Ok(None);
    };
    // The extensions dir may not exist yet even when pi does; its parent (the
    // agent dir) existing is the sign pi is set up, the same test herdr's own
    // installer applies before creating the extensions dir.
    let Some(parent) = dir.parent() else {
        return Ok(None);
    };
    if !parent.exists() {
        return Ok(None);
    }
    write_extension(&dir)?;
    Ok(Some(
        "activity extension — reports the prompt and each tool call".to_string(),
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
        &format!("AMON_OMP_ACTIVITY_VERSION={VERSION}"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_asset_stamps_the_version_state_checks_for() {
        assert!(
            ASSET.contains(&format!("AMON_OMP_ACTIVITY_VERSION={VERSION}")),
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
        for event in ["before_agent_start", "tool_execution_start", "message_end"] {
            assert!(ASSET.contains(event), "missing subscription: {event}");
        }
    }

    #[test]
    fn installing_writes_the_file_and_uninstall_is_removal() {
        let dir = tempfile::tempdir().expect("tempdir");
        let extensions = dir.path().join("extensions");
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
