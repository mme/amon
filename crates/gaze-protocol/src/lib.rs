//! Wire types for every socket in gaze.
//!
//! Two sockets speak this protocol: the daemon's (wrappers, subscribers and
//! `gaze status` connect to it) and each wrapper's own (installed integration
//! hooks connect to it). Both carry newline-delimited JSON.
//!
//! # Compatibility
//!
//! An old wrapper must keep working against a newer daemon forever, so the
//! protocol only ever grows: fields are added, never removed or retyped, and
//! readers are tolerant. Unknown fields are ignored, unknown methods are
//! answered with an [`ErrorCode::UnsupportedMethod`] response rather than
//! dropping the connection, and unknown events deserialize to
//! [`Event::Unknown`] so a subscriber built against an older release simply
//! skips what it does not recognize.

mod agent;
mod event;
mod frame;
mod method;
mod schema;

pub use agent::{AgentEntry, AgentPatch, AgentState};
pub use event::Event;
pub use frame::{Error, ErrorCode, ParseError, Request, Response, ServerFrame};
pub use method::{
    Hello, HelloResult, Method, MethodError, ReportSession, ReportState, Role, StatusResult,
};
pub use schema::protocol_schema;

/// Bumped only for changes an old peer cannot tolerate. Additive changes —
/// which is all we intend to make — leave this alone.
pub const PROTOCOL_VERSION: u32 = 1;

/// Environment variables the wrapper injects into the agent it spawns, so that
/// integration hooks can find their way back to it. Mirrors herdr's
/// `HERDR_ENV` / `HERDR_SOCKET_PATH` / `HERDR_PANE_ID` contract.
pub mod env {
    /// Set to `"1"`. Hooks exit silently unless this is present.
    pub const GAZE_ENV: &str = "GAZE_ENV";
    /// Path to the wrapper's own hook socket.
    pub const SOCKET_PATH: &str = "GAZE_SOCKET_PATH";
    /// The registry id of the agent the hook is reporting about.
    pub const AGENT_ID: &str = "GAZE_AGENT_ID";
    /// Value of [`GAZE_ENV`] when gaze is wrapping the process.
    pub const GAZE_ENV_VALUE: &str = "1";
}
