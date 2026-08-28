//! The shadow terminal and the agent-state machine it feeds.
//!
//! Two halves: [`ShadowTerminal`] is ours, a headless Ghostty terminal that
//! mirrors what the user sees; everything under `vendor/` is herdr's terminal
//! state and effective-state arbitration, copied verbatim (ADR-0001, ADR-0005).
//! Arbitration is why hook reports and screen detection have to meet in the
//! wrapper: `TerminalState` wants both.
//!
//! The shims recreate herdr's crate-internal namespace so the vendored files
//! compile untouched. `crate::detect` is the vendored engine re-exported from
//! `amon-detect`, not a copy of it.

#[allow(dead_code, clippy::too_many_arguments)]
#[path = "vendor/agent_resume.rs"]
pub(crate) mod agent_resume;
#[allow(dead_code)]
#[path = "vendor/metadata_tokens.rs"]
pub(crate) mod metadata_tokens;
#[path = "shim/platform.rs"]
pub(crate) mod platform;
// `private_interfaces`: herdr exposes pub(crate) types on pub fields, which is
// fine inside one crate and harmless here. ADR-0005 keeps the allow out of the
// vendored file.
#[allow(dead_code, private_interfaces, clippy::too_many_arguments)]
#[path = "shim/terminal.rs"]
pub mod terminal;

pub use amon_detect;

pub(crate) use amon_detect::detect;

mod shadow;

// The wrapper builds session refs and start sources from hook reports the
// same way herdr does, so hook authority is scoped to the session that
// reported it and session reports reanchor the state machine.
pub use agent_resume::{normalize_session_start_source, session_ref_from_report, AgentSessionRef};
pub use shadow::ShadowTerminal;
pub use terminal::{EffectiveStateChange, TerminalId, TerminalState, TerminalStateMutation};
