//! Agent state detection, vendored from herdr.
//!
//! The engine and its manifests are copied verbatim (see ADR-0001 and
//! ADR-0005); nothing under `vendor/` is hand-edited. The vendored files
//! reference herdr's crate-internal modules — `crate::config`,
//! `crate::platform`, and so on — so this crate recreates that namespace with
//! the shims below, which is what lets them compile untouched.

// Shim module names are herdr's, not ours, and are load-bearing: renaming one
// breaks the vendored code that imports it. They expose the full surface the
// vendored code may reference, so parts go unused — as does much of the
// vendored code itself, since amon uses a subset of what a multiplexer needs.
// ADR-0005 forbids trimming either, so the allows sit here rather than in any
// vendored file.
#[allow(dead_code)]
#[path = "shim/config.rs"]
pub(crate) mod config;
#[allow(dead_code)]
#[path = "shim/events.rs"]
pub(crate) mod events;
#[allow(dead_code)]
#[path = "shim/noninteractive_process.rs"]
pub(crate) mod noninteractive_process;
#[allow(dead_code)]
#[path = "shim/pane.rs"]
pub(crate) mod pane;
#[allow(dead_code)]
#[path = "shim/platform.rs"]
pub(crate) mod platform;

#[allow(dead_code, irrefutable_let_patterns, clippy::too_many_arguments)]
#[path = "vendor/detect/mod.rs"]
pub mod detect;

// `detect_state` is deliberately absent: upstream marks it `#[cfg(test)]` as a
// convenience wrapper. The real entry point is `detect_agent_with_osc`, which
// also carries the confidence metadata arbitration needs.
pub use detect::{
    agent_label, detect_agent, detect_agent_with_osc, identify_agent, parse_agent_label, Agent,
    AgentDetection, AgentState,
};

/// Refreshing the bundled manifests from the catalog.
///
/// The vendored updater is crate-private, and re-exporting it would mean
/// patching upstream. Since amon's only caller is the daemon, a small wrapper
/// here is cheaper than a patch and keeps ADR-0005's rule intact.
pub mod manifests {
    /// Fetches the catalog and installs any manifests newer than what is on
    /// disk, returning how many were replaced. Manifests requiring a newer
    /// engine than this build are refused by the updater.
    pub fn check_and_update() -> Result<usize, String> {
        crate::detect::manifest_update::check_and_update().map(|output| output.updated.len())
    }
}
