//! amon's own activity hooks for harnesses beyond Claude (ADR-0021: seams).
//!
//! Claude's prompt hook lives in `crate::prompt_hook` and edits a settings
//! file the user also writes, which is why it goes through herdr's
//! formatting-preserving CST splice. The harnesses here are simpler surfaces:
//! Codex's `hooks.json` is machine-owned — herdr itself rewrites it whole with
//! a pretty-printer — and pi and omp load any `.ts` file dropped into their
//! extension directory, so installing is writing one file and uninstalling is
//! removing it.
//!
//! Everything here follows the seam obligations: installed after herdr's own
//! install for the target, idempotent, non-fatal on failure (the screen or
//! plain state detection is the fallback), and removed by amon's own
//! uninstall so nothing is orphaned.

pub mod codex;

/// Whether an installed piece is absent, stale, or current — what `doctor`
/// reports for each of these hooks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookState {
    NotInstalled,
    Outdated,
    Current,
}

pub(crate) fn state_of(path: &std::path::Path, stamp: &str) -> std::io::Result<HookState> {
    if !path.exists() {
        return Ok(HookState::NotInstalled);
    }
    let installed = std::fs::read_to_string(path)?;
    Ok(if installed.contains(stamp) {
        HookState::Current
    } else {
        HookState::Outdated
    })
}

#[cfg(unix)]
pub(crate) fn set_executable(path: &std::path::Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
}

#[cfg(not(unix))]
pub(crate) fn set_executable(_path: &std::path::Path) -> std::io::Result<()> {
    Ok(())
}
