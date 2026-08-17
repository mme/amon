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
    integration::install_target(target)
}

/// Reverts exactly what [`install`] did.
pub fn uninstall(target: IntegrationTarget) -> io::Result<Vec<String>> {
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

/// Every agent worth offering: the union of Detected Agents and installed
/// Integrations. Agents that are neither do not appear — `amon setup <name>`
/// remains the escape hatch for pre-installing one.
pub fn candidates() -> Vec<Candidate> {
    integration::integration_recommendations()
        .into_iter()
        .filter(|recommendation| recommendation.available)
        .map(|recommendation| Candidate {
            target: recommendation.target,
            label: recommendation.label,
            // A candidate that is not installed is only here because the
            // registry detected it, so it is a Detected Agent by definition —
            // layout special cases (a standalone codex, hermes) included. For
            // installed rows `available` is always true and can no longer
            // tell, so a PATH scan over the agent's command names refines the
            // orphan flag; at worst that flag reads "agent not found" for an
            // agent installed some exotic way.
            detected: recommendation.state == integration::IntegrationStatusKind::NotInstalled
                || command_names(recommendation.target)
                    .iter()
                    .any(|command| on_path(command)),
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

pub fn all_targets() -> [IntegrationTarget; 15] {
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
