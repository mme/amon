//! Wire types for every socket in amon.
//!
//! Two sockets speak this protocol: the daemon's (wrappers, subscribers and
//! `amon status` connect to it) and each wrapper's own (installed integration
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
mod config;
mod connect;
mod event;
mod frame;
mod method;
pub mod paths;
mod schema;

pub use agent::{Activity, ActivityKind, AgentEntry, AgentPatch, AgentState, Runtime};
pub use config::{BarConfig, Config, GlyphConfig, SoundConfig, UpdatesConfig};
pub use connect::connect_or_spawn_daemon;
pub use event::Event;
pub use frame::{Error, ErrorCode, ParseError, Request, Response, ServerFrame};
pub use method::{
    ConfigResult, Hello, HelloResult, Method, MethodError, ReportSession, ReportState, Role,
    StatusResult,
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
    pub const AMON_ENV: &str = "AMON_ENV";
    /// Path to the wrapper's own hook socket.
    pub const SOCKET_PATH: &str = "AMON_SOCKET_PATH";
    /// The registry id of the agent the hook is reporting about.
    pub const AGENT_ID: &str = "AMON_AGENT_ID";
    /// Value of [`AMON_ENV`] when amon is wrapping the process.
    pub const AMON_ENV_VALUE: &str = "1";
    /// The pid of the wrapper that spawned this process.
    ///
    /// Not for hooks — for the shims, which need to tell "amon is the process
    /// that just called me" from "amon is somewhere above me". Every variable
    /// here descends the whole process tree, so presence alone cannot carry
    /// that distinction and a pid to compare against `$PPID` can (ADR-0016).
    pub const WRAPPER_PID: &str = "AMON_WRAPPER_PID";
    /// Set to `"1"` by an agent runtime in every pane process it manages —
    /// herdr's `HERDR_ENV`, luvus's `LUVUS_ENV`. Their contracts, not ours:
    /// the exact names each documents for its integrations. The wrapper steps
    /// aside when any is set, because inside a runtime the runtime is the
    /// detection authority (ADR-0016, and the daemon's runtime seam).
    pub const RUNTIME_PANE_ENVS: &[&str] = &["HERDR_ENV", "LUVUS_ENV"];
}
