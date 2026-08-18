//! Per-agent integration install, uninstall, and status — vendored from herdr.
//!
//! `amon setup claude` writes a version-stamped hook script into the agent's
//! own config directory and edits that agent's config to register it, without
//! disturbing anything the user put there. Reinstalling overwrites only the
//! managed file. This is herdr's machinery unchanged (ADR-0004): the surgical
//! config editing is the hard, well-tested part, and reimplementing it would
//! only re-earn herdr's bug fixes.
//!
//! The hook assets are the same too, with the herdr → amon rename applied by
//! `scripts/revendor.sh`. They report to the wrapper that spawned the agent
//! rather than to a central daemon (ADR-0001), which the assets do not need to
//! know: they read their socket path from the environment.

// Shim module names are herdr's and are load-bearing; see ADR-0005.
#[allow(dead_code)]
#[path = "shim/api.rs"]
pub(crate) mod api;
#[allow(dead_code)]
#[path = "shim/logging.rs"]
pub(crate) mod logging;
#[allow(dead_code)]
#[path = "shim/noninteractive_process.rs"]
pub(crate) mod noninteractive_process;

// `unused_imports`: the vendored module re-exports its full surface; amon uses
// a subset via api_surface. ADR-0005 keeps the allow out of the vendored file.
#[allow(dead_code, unused_imports, clippy::too_many_arguments)]
#[path = "vendor/integration/mod.rs"]
pub mod integration;

pub mod alias;
mod api_surface;
pub mod desktop;

pub use api::schema::IntegrationTarget;
pub use api_surface::{
    all_targets, candidates, command_names, install, outdated_notice, parse_target, statuses,
    target_label, uninstall, Candidate, InstallState, Status,
};
pub use desktop::{DesktopStatus, DesktopTarget};
