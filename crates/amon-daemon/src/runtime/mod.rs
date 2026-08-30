//! Agents living inside agent runtimes — herdr, luvus — adopted without
//! wrapping the runtime.
//!
//! Each runtime is a detached server owning the agents' terminals, a
//! tty-owning client the user typed, and a JSON-lines control socket. What
//! differs between them — how their processes are told apart, where the
//! socket is, how the socket is spoken to, how events are read — is the
//! [`Hosted`] trait; finding attached clients, projecting agents into the
//! registry, and following their window are the same code for all.
//!
//! See docs/research/herdr-live-integration.md for the shape of the whole
//! thing and the decisions this implements, and ADR-0018 for the seam.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

pub mod discover;
pub mod herdr;
pub mod luvus;
pub mod project;
mod supervisor;
pub mod wire;

pub use supervisor::spawn;

/// What one runtime process is, judged from its kernel-visible facts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Role {
    Client { name: Option<String> },
    Server { name: Option<String> },
    Other,
}

/// One runtime session with an attached client — the only kind amon shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Session {
    pub name: Option<String>,
    /// The session's JSON API socket.
    pub socket: PathBuf,
    /// The attached client whose window the session's agents borrow.
    pub client_pid: u32,
}

/// An agent as the runtime reports it, in the shape the projection needs.
/// Everything else on the wire is ignored, which is what keeps this
/// compatible across the runtimes' releases.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct HostedAgent {
    /// Stable within one life of the agent: herdr's terminal id, luvus's
    /// pane id. A pane move must not read as an agent leaving and another
    /// arriving, so this is never the thing that moves.
    pub identity: String,
    /// The pane currently hosting the agent — what the focus hop names.
    pub pane: String,
    pub agent: Option<String>,
    pub status: String,
    pub cwd: Option<String>,
    /// A per-life counter (herdr's `state_change_seq`), which going
    /// backwards is the only way to tell a restart in the same terminal.
    /// None where the runtime keeps no such counter.
    pub life: Option<u64>,
}

/// One pushed event, as the supervisor reads it.
#[derive(Debug, PartialEq)]
pub enum HostedEvent {
    /// A status change on an agent already known. `agent`: None when the
    /// event did not say; Some(None) when the runtime has no name for what
    /// is in the pane now.
    Status {
        pane: String,
        status: String,
        agent: Option<Option<String>>,
    },
    /// The agent in `pane` is gone — exited, or replaced by another. Its
    /// claim is retired before the resnapshot, so whatever the snapshot
    /// finds there is a first claim with its own clocks, not a continuation
    /// of a life that has ended.
    Departed { pane: String },
    /// Anything else that changes the set of agents or panes: resnapshot.
    Structural,
    /// Not about agents at all.
    Ignore,
}

/// What differs between runtimes. Everything else — discovery, the
/// supervisor, projection — is generic over this.
pub trait Hosted: Send + 'static {
    /// The id prefix and the `kind` on the wire.
    const KIND: &'static str;
    /// Process comm of both the client and the server.
    const COMM: &'static str;
    /// Whether status events are subscribed per pane, so that a change to
    /// the set of agent panes needs a fresh subscription.
    const STATUS_PER_PANE: bool;

    fn classify(argv: &[String], is_session_leader: bool, has_tty: bool, environ: &str) -> Role;
    fn socket_for(argv: &[String], environ: &str) -> Option<PathBuf>;

    fn new() -> Self;
    /// Subscribe — given the panes currently owned — then snapshot. Returns
    /// the agents now and the stream of what changes.
    fn attach(
        &mut self,
        socket: &Path,
        panes: &[String],
    ) -> std::io::Result<(Vec<HostedAgent>, wire::EventStream)>;
    /// Reads one pushed event, given pane → agent label for owned panes.
    fn event(&mut self, event: &wire::RawEvent, owned: &HashMap<String, String>) -> HostedEvent;
    fn runtime(session: &Session, agent: &HostedAgent) -> amon_protocol::Runtime;
}
