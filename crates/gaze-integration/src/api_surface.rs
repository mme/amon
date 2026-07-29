//! The public face of the vendored installer.
//!
//! Everything here is a thin pass-through: the behaviour is herdr's, and this
//! exists so the rest of gaze depends on a small stable surface instead of
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
            "gaze: {agent} integration is outdated (v{} < v{}); run `gaze install {agent}`",
            status
                .installed_version
                .map(|version| version.to_string())
                .unwrap_or_else(|| "?".into()),
            status.expected_version
        )
    })
}
