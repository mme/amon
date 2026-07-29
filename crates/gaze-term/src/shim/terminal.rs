//! Mounts herdr's vendored terminal-state files under the module path they
//! expect (`crate::terminal`). herdr's own `terminal/mod.rs` also declares the
//! pane-spawning runtime, which gaze does not vendor — it spawns its own PTY —
//! so this is our file rather than a vendored one.

#[path = "../vendor/terminal/id.rs"]
mod id;
#[path = "../vendor/terminal/state.rs"]
pub mod state;
#[path = "../vendor/terminal/title.rs"]
mod title;

pub use id::TerminalId;
pub use state::{
    AgentMetadataReport, EffectivePresentation, EffectiveStateChange, TerminalState,
    TerminalStateMutation,
};
pub(crate) use title::stripped_terminal_title;
