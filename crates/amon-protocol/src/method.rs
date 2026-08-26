use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::agent::{AgentEntry, AgentPatch, AgentState};
use crate::config::Config;

/// What a peer is, declared in its [`Hello`].
///
/// Declarative, not load-bearing: what a connection can do is decided by the
/// methods it sends (`agent.register` claims an entry, `subscribe` starts the
/// event stream), so the daemon does not branch on this. It exists for
/// diagnostics and for future daemons that may want to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Wrapper,
    Subscriber,
    Client,
    /// An installed integration hook, talking to a wrapper rather than the
    /// daemon. Hooks do not actually send a hello — they send one request and
    /// hang up — but the role exists so their frames can be described.
    Hook,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Hello {
    pub role: Role,
    pub protocol: u32,
    /// Version of the amon binary on the connecting side.
    pub version: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct HelloResult {
    pub protocol: u32,
    /// Version of the amon binary running the daemon. A client with a newer
    /// version uses this to decide to take the daemon over.
    pub version: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct StatusResult {
    pub agents: Vec<AgentEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ConfigResult {
    pub config: Config,
    /// Why the file on disk is not what `config` says, when it is not: a
    /// parse failure keeps the last good configuration (ADR-0012), and this
    /// is where that state travels so `amon doctor` can report it. Absent
    /// when the file parsed or simply does not exist.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// A hook reporting agent state. Field names mirror herdr's `pane.report_agent`
/// so that vendored hook assets need only mechanical renaming.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ReportState {
    /// The `AMON_AGENT_ID` the hook was given.
    pub agent_id: String,
    /// Which integration produced this, e.g. `amon:claude`.
    pub source: String,
    pub agent: String,
    pub state: AgentState,
    /// Monotonic per-source sequence number, used to drop out-of-order reports.
    pub seq: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_session_id: Option<String>,
}

/// A hook reporting session identity. Mirrors herdr's
/// `pane.report_agent_session`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ReportSession {
    pub agent_id: String,
    pub source: String,
    pub agent: String,
    pub seq: u64,
    pub agent_session_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_session_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_start_source: Option<String>,
}

/// Every request either socket accepts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "method", content = "params")]
pub enum Method {
    /// Opens any connection to the daemon.
    #[serde(rename = "hello")]
    Hello(Hello),
    /// Wrapper claims a registry entry. The entry lives until this connection
    /// drops.
    #[serde(rename = "agent.register")]
    AgentRegister(AgentEntry),
    /// Wrapper reports a change to its own entry.
    #[serde(rename = "agent.update")]
    AgentUpdate(AgentPatch),
    /// Lists connected agents.
    #[serde(rename = "status")]
    Status,
    /// The user's configuration, as the daemon last read it (ADR-0012).
    #[serde(rename = "config")]
    Config,
    /// Turns this connection into an event stream.
    #[serde(rename = "subscribe")]
    Subscribe,
    /// Asks an older daemon to exit so a newer binary can take over.
    #[serde(rename = "daemon.shutdown")]
    DaemonShutdown,
    /// Hook → wrapper.
    #[serde(rename = "agent.report_state")]
    AgentReportState(ReportState),
    /// Hook → wrapper.
    #[serde(rename = "agent.report_session")]
    AgentReportSession(ReportSession),
}

impl Method {
    pub fn name(&self) -> &'static str {
        match self {
            Self::Hello(_) => "hello",
            Self::AgentRegister(_) => "agent.register",
            Self::AgentUpdate(_) => "agent.update",
            Self::Status => "status",
            Self::Config => "config",
            Self::Subscribe => "subscribe",
            Self::DaemonShutdown => "daemon.shutdown",
            Self::AgentReportState(_) => "agent.report_state",
            Self::AgentReportSession(_) => "agent.report_session",
        }
    }

    /// Builds a method from an envelope's `method` and `params`.
    ///
    /// Parsing the envelope and parsing the method are deliberately separate:
    /// an unrecognized or malformed method still leaves us holding the request
    /// id, so we can answer with an error instead of dropping the connection.
    pub fn parse(name: &str, params: serde_json::Value) -> Result<Self, MethodError> {
        let tagged = serde_json::json!({ "method": name, "params": params });
        serde_json::from_value(tagged).map_err(|source| {
            if KNOWN_METHODS.contains(&name) {
                MethodError::InvalidParams(source.to_string())
            } else {
                MethodError::Unsupported(name.to_string())
            }
        })
    }
}

const KNOWN_METHODS: &[&str] = &[
    "hello",
    "agent.register",
    "agent.update",
    "status",
    "config",
    "subscribe",
    "daemon.shutdown",
    "agent.report_state",
    "agent.report_session",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MethodError {
    Unsupported(String),
    InvalidParams(String),
}

impl std::fmt::Display for MethodError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unsupported(name) => write!(f, "unsupported method: {name}"),
            Self::InvalidParams(detail) => write!(f, "invalid params: {detail}"),
        }
    }
}

impl std::error::Error for MethodError {}
