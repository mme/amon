//! One hosted agent, as an amon entry.
//!
//! The state mapping is the load-bearing part: a runtime's `done` is amon's
//! Idle-with-seen-false (finished but unnoticed), and both sides mean the
//! same thing by it because amon vendors herdr's detector (ADR-0001) and
//! luvus's states are the same five. An unrecognized status maps to
//! Unknown, which ranks last — a new status must not cry wolf on every bar.

use amon_protocol::{AgentEntry, AgentState};

use super::{Hosted, HostedAgent, Session};

/// What an agent the runtime has not named is called. herdr's schema allows
/// a null agent, and a blank first column reads as a bug rather than as a
/// fact.
pub const UNNAMED: &str = "unknown";

/// A short, stable digest of the session's socket path.
///
/// The socket is what makes a session a session — two clients on one socket
/// are one session, two nameless clients on different sockets are two — so
/// it, and not the display name, is what keeps public agent ids apart.
/// Sessions can share a name across config homes, and terminal ids are only
/// unique within the server that issued them; an id built from the name
/// would let one session's row overwrite another's, and send a jump to the
/// wrong window. FNV-1a because this needs to be stable and short, not
/// cryptographic.
fn socket_digest(socket: &std::path::Path) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in socket.as_os_str().as_encoded_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")
}

pub fn state_for(status: &str) -> (AgentState, Option<bool>) {
    match status {
        "idle" => (AgentState::Idle, Some(true)),
        "done" => (AgentState::Idle, Some(false)),
        "working" => (AgentState::Working, None),
        "blocked" => (AgentState::Blocked, Some(false)),
        _ => (AgentState::Unknown, None),
    }
}

pub fn entry_for<H: Hosted>(
    agent: &HostedAgent,
    session: &Session,
    window: Option<&amon_hypr::Window>,
    now_ms: u64,
) -> AgentEntry {
    let (state, seen) = state_for(&agent.status);
    // The runtime may hold an agent it has no name for; the row still
    // belongs on the bar, labelled for what it is rather than dropped.
    let label = agent.agent.clone().unwrap_or_else(|| UNNAMED.to_string());
    AgentEntry {
        id: format!(
            "{}:{}:{}",
            H::KIND,
            socket_digest(&session.socket),
            agent.identity
        ),
        agent: label.clone(),
        state,
        state_since: now_ms,
        cwd: agent.cwd.clone().unwrap_or_default(),
        // Not a wrapped process; there is no agent pid amon vouches for.
        pid: 0,
        args: vec![label],
        hostname: hostname(),
        started_at: now_ms,
        agent_session_id: None,
        agent_session_path: None,
        window: window.map(|window| window.address.clone()),
        workspace: window.and_then(|window| window.workspace.clone()),
        // No branch and no Project for a hosted agent. Both are facts about
        // a directory, and amon could read them from the cwd the runtime
        // reports — but neither is watched here, and giving a row half the
        // git facts a wrapped row has would read as amon knowing less than it
        // does rather than as the honest gap it is. Filling these in belongs
        // with teaching this path to watch a directory at all.
        branch: None,
        project: None,
        subpath: None,
        focused: None,
        seen,
        runtime: Some(H::runtime(session, agent)),
    }
}

fn hostname() -> String {
    let mut buffer = [0u8; 256];
    if unsafe { libc::gethostname(buffer.as_mut_ptr().cast(), buffer.len()) } != 0 {
        return String::new();
    }
    let end = buffer.iter().position(|byte| *byte == 0).unwrap_or(0);
    String::from_utf8_lossy(&buffer[..end]).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::herdr::Herdr;
    use amon_protocol::{AgentState, Runtime};

    fn agent(status: &str) -> HostedAgent {
        HostedAgent {
            identity: "t1".into(),
            pane: "w1:p2".into(),
            agent: Some("claude".into()),
            status: status.into(),
            cwd: Some("/repo".into()),
            life: None,
        }
    }

    fn session() -> Session {
        Session {
            name: Some("work".into()),
            socket: "/tmp/herdr.sock".into(),
            client_pid: 4242,
        }
    }

    #[test]
    fn runtime_statuses_map_onto_amon_states_and_seen() {
        assert_eq!(state_for("idle"), (AgentState::Idle, Some(true)));
        assert_eq!(state_for("done"), (AgentState::Idle, Some(false)));
        assert_eq!(state_for("working"), (AgentState::Working, None));
        assert_eq!(state_for("blocked"), (AgentState::Blocked, Some(false)));
        assert_eq!(state_for("unknown"), (AgentState::Unknown, None));
        // A status this build has never heard of ranks last, never first.
        assert_eq!(state_for("meditating"), (AgentState::Unknown, None));
    }

    #[test]
    fn an_entry_carries_identity_window_and_the_runtime_pointer() {
        let window = amon_hypr::Window {
            address: "abc123".into(),
            workspace: Some("3".into()),
        };
        let entry = entry_for::<Herdr>(&agent("done"), &session(), Some(&window), 1000);
        assert!(entry.id.starts_with("herdr:"));
        assert!(entry.id.ends_with(":t1"));
        assert_eq!(entry.agent, "claude");
        assert_eq!(entry.state, AgentState::Idle);
        assert_eq!(entry.seen, Some(false));
        assert_eq!(entry.state_since, 1000);
        assert_eq!(entry.cwd, "/repo");
        assert_eq!(entry.window.as_deref(), Some("abc123"));
        assert_eq!(entry.workspace.as_deref(), Some("3"));
        assert!(!entry.hostname.is_empty());
        let runtime = entry.runtime.expect("projected entries carry provenance");
        assert_eq!(
            runtime,
            Runtime::Herdr {
                socket: "/tmp/herdr.sock".into(),
                session: Some("work".into()),
                pane: "w1:p2".into(),
            }
        );
    }

    #[test]
    fn an_agent_the_runtime_cannot_name_is_labelled_rather_than_blank() {
        let unnamed = HostedAgent {
            agent: None,
            cwd: None,
            ..agent("blocked")
        };
        let entry = entry_for::<Herdr>(&unnamed, &session(), None, 1);
        assert_eq!(entry.agent, "unknown");
        assert_eq!(entry.args, vec!["unknown".to_string()]);
        assert_eq!(entry.cwd, "");
        // The row still ranks and jumps like any other blocked agent.
        assert_eq!(entry.state, AgentState::Blocked);
    }

    #[test]
    fn a_nameless_session_still_projects() {
        let mut session = session();
        session.name = None;
        let entry = entry_for::<Herdr>(&agent("idle"), &session, None, 1);
        assert!(entry.id.ends_with(":t1"));
        assert_eq!(entry.window, None);
        assert_eq!(entry.workspace, None);
        assert!(matches!(
            entry.runtime,
            Some(Runtime::Herdr { session: None, .. })
        ));
    }

    #[test]
    fn two_sessions_sharing_a_name_do_not_share_agent_ids() {
        // Same name, same terminal id, different sockets: two servers, two
        // agents. The id must tell them apart or one row overwrites the
        // other and a jump lands in the wrong window.
        let mut other = session();
        other.socket = "/tmp/elsewhere/herdr.sock".into();
        let one = entry_for::<Herdr>(&agent("idle"), &session(), None, 1);
        let two = entry_for::<Herdr>(&agent("idle"), &other, None, 1);
        assert_ne!(one.id, two.id);
    }
}
