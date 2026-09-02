//! The public face of the vendored installer.
//!
//! Everything here is a thin pass-through: the behaviour is herdr's, and this
//! exists so the rest of amon depends on a small stable surface instead of
//! reaching into vendored internals that move when we re-vendor.

use std::io;
use std::path::PathBuf;

use crate::api::schema::IntegrationTarget;
use crate::integration;

/// Installs the hook for one agent, returning the human-readable notes to
/// print. Idempotent: reinstalling replaces the managed file and leaves
/// anything else in the agent's config alone.
pub fn install(target: IntegrationTarget) -> io::Result<Vec<String>> {
    let mut notes = integration::install_target(target)?;
    // amon's own additions run after herdr's install, which for Claude
    // removes the very event the prompt hook needs (ADR-0021). Idempotent, so
    // safe on every setup and upgrade.
    //
    // Non-fatal on purpose: the prompt hook is an enhancement over the screen
    // fallback, and herdr's state hook has already installed by now. A failure
    // here — an unwritable settings file, say — must not fail the whole
    // integration and take the alias down with it; it degrades to the screen.
    let extra: Option<Result<Option<String>, io::Error>> = match target {
        IntegrationTarget::Claude => Some(crate::prompt_hook::install()),
        IntegrationTarget::Codex => Some(crate::activity_hooks::codex::install()),
        IntegrationTarget::Pi => Some(crate::activity_hooks::pi::install()),
        _ => None,
    };
    if let Some(result) = extra {
        match result {
            Ok(Some(note)) => notes.push(note),
            Ok(None) => {}
            Err(error) => notes.push(format!(
                "prompt hook not installed ({error}); turns still read from the screen"
            )),
        }
    }
    Ok(notes)
}

/// Reverts exactly what [`install`] did.
pub fn uninstall(target: IntegrationTarget) -> io::Result<Vec<String>> {
    // What amon added, amon removes (ADR-0021) — before herdr's uninstall, so
    // the settings file is edited by our step while its hooks object is still
    // whole. Best-effort: a failure here must not stop herdr's uninstall from
    // running, or a remove could strand the state hook it was meant to take.
    match target {
        IntegrationTarget::Claude => {
            let _ = crate::prompt_hook::uninstall();
        }
        IntegrationTarget::Codex => {
            let _ = crate::activity_hooks::codex::uninstall();
        }
        IntegrationTarget::Pi => {
            let _ = crate::activity_hooks::pi::uninstall();
        }
        _ => {}
    }
    integration::uninstall_target(target)
}

/// The commands an agent is actually run as, most usual first.
///
/// Not the same as the target's label: `cursor` is run as `cursor-agent`, and
/// Kilo answers to two names. Aliases need the real ones, and herdr already
/// keeps that table.
pub fn command_names(target: IntegrationTarget) -> &'static [&'static str] {
    integration::integration_target_command_names(target)
}

/// Whether an agent's installed hook is current, outdated, or absent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallState {
    NotInstalled,
    Current,
    Outdated,
}

#[derive(Debug, Clone)]
pub struct Status {
    pub target: IntegrationTarget,
    pub label: &'static str,
    pub path: PathBuf,
    pub state: InstallState,
    pub installed_version: Option<u32>,
    pub expected_version: u32,
}

pub fn statuses() -> Vec<Status> {
    integration::installed_integration_statuses()
        .into_iter()
        .map(|status| Status {
            target: status.target,
            label: integration::integration_target_label(status.target),
            path: status.path,
            state: match status.state {
                integration::IntegrationStatusKind::NotInstalled => InstallState::NotInstalled,
                integration::IntegrationStatusKind::Current => InstallState::Current,
                integration::IntegrationStatusKind::Outdated => InstallState::Outdated,
            },
            installed_version: status.installed_version,
            expected_version: status.expected_version,
        })
        .collect()
}

pub fn target_label(target: IntegrationTarget) -> &'static str {
    integration::integration_target_label(target)
}

/// One row of what `amon setup` has to offer: a Detected Agent, an installed
/// Integration, or both at once.
#[derive(Debug, Clone)]
pub struct Candidate {
    pub target: IntegrationTarget,
    pub label: &'static str,
    /// Whether the agent's own CLI is on this machine. An installed
    /// Integration whose agent has vanished stays a Candidate — flagged, so
    /// the one screen can still remove it — which is why this is separate
    /// from [`Candidate::state`].
    pub detected: bool,
    pub state: InstallState,
}

/// Whether the agent whose hook belongs at `hook` is actually on this machine.
///
/// Not a PATH scan, which is what upstream's `available` does and what amon
/// used to pass through. On Omarchy that answer is always yes: every major
/// agent CLI is pre-wired as a lazy mise stub in `~/.local/bin`, downloading
/// nothing until its first run. So `grok`, `copilot` and `omp` are on PATH from
/// a fresh install, the screen offered to hook all three, and each one failed
/// with "config directory not found. install it first" — offering work that
/// cannot succeed on the one platform amon is built for.
///
/// The honest signal is the directory the installer itself insists on. Rather
/// than restate each target's rule — they live in vendored installers this
/// crate must not reach into (ADR-0005) — take the agent's own root: the
/// ancestor of the hook path sitting directly under `$HOME` or the config dir.
/// `~/.claude/hooks/…` gives `~/.claude`, `~/.config/opencode/plugins/…` gives
/// `~/.config/opencode`, and both are exactly what those installers check.
///
/// Two known imprecisions, both quieter than the bug they replace. An agent
/// whose root exists but whose deeper directory does not (pi wants
/// `~/.pi/agent`) still reads as present and still fails at install. And
/// mastracode, whose installer has no precondition at all, reads as absent
/// until it has a directory — `amon setup mastracode` remains the way in.
fn agent_root_exists(hook: &std::path::Path) -> bool {
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
        return false;
    };
    let config = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".config"));

    let mut root = hook;
    while let Some(parent) = root.parent() {
        if parent == home || parent == config {
            return root.is_dir();
        }
        root = parent;
    }
    false
}

/// Every agent worth offering: the union of Detected Agents and installed
/// Integrations. Agents that are neither do not appear — `amon setup <name>`
/// remains the escape hatch for pre-installing one.
pub fn candidates() -> Vec<Candidate> {
    integration::integration_recommendations()
        .into_iter()
        .filter(|recommendation| {
            recommendation.state != integration::IntegrationStatusKind::NotInstalled
                || agent_root_exists(&recommendation.path)
        })
        .map(|recommendation| Candidate {
            target: recommendation.target,
            label: recommendation.label,
            detected: agent_root_exists(&recommendation.path),
            state: match recommendation.state {
                integration::IntegrationStatusKind::NotInstalled => InstallState::NotInstalled,
                integration::IntegrationStatusKind::Current => InstallState::Current,
                integration::IntegrationStatusKind::Outdated => InstallState::Outdated,
            },
        })
        .collect()
}

pub(crate) fn on_path(command: &str) -> bool {
    let Some(path) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&path).any(|dir| is_executable(&dir.join(command)))
}

#[cfg(unix)]
fn is_executable(path: &std::path::Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .map(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn is_executable(path: &std::path::Path) -> bool {
    path.is_file()
}

/// Parses a CLI argument like `claude` into a target.
pub fn parse_target(name: &str) -> Option<IntegrationTarget> {
    IntegrationTarget::ALL
        .into_iter()
        .find(|target| target_label(*target) == name)
}

pub fn all_targets() -> [IntegrationTarget; 17] {
    IntegrationTarget::ALL
}

/// The one-line nudge the wrapper prints to stderr before launching an agent
/// whose installed hook predates this build.
pub fn outdated_notice(agent: &str) -> Option<String> {
    let target = parse_target(agent)?;
    let status = statuses()
        .into_iter()
        .find(|status| status.target == target)?;
    (status.state == InstallState::Outdated).then(|| {
        format!(
            "amon: {agent} integration is outdated (v{} < v{}); run `amon setup {agent}`",
            status
                .installed_version
                .map(|version| version.to_string())
                .unwrap_or_else(|| "?".into()),
            status.expected_version
        )
    })
}
