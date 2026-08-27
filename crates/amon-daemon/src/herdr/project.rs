//! One herdr agent, as an amon entry.
//!
//! The state mapping is the load-bearing part: herdr's `done` is amon's
//! Idle-with-seen-false (finished but unnoticed), and both sides mean the
//! same thing by it because amon vendors herdr's detector (ADR-0001). An
//! unrecognized status maps to Unknown, which ranks last — a new herdr
//! status must not cry wolf on every bar.

use amon_protocol::{AgentEntry, AgentState, HerdrInfo};

use super::proc::HerdrSession;
use super::wire::HerdrAgent;

/// What an agent herdr has not named is called. herdr's schema allows a null
/// agent, and a blank first column reads as a bug rather than as a fact.
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

pub fn entry_for(
    agent: &HerdrAgent,
    session: &HerdrSession,
    window: Option<&amon_hypr::Window>,
    now_ms: u64,
) -> AgentEntry {
    let (state, seen) = state_for(&agent.agent_status);
    // herdr may hold an agent it has no name for; the row still belongs on
    // the bar, labelled for what it is rather than dropped.
    let label = agent.agent.clone().unwrap_or_else(|| UNNAMED.to_string());
    AgentEntry {
        id: format!(
            "herdr:{}:{}",
            socket_digest(&session.socket),
            agent.terminal_id
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
        title: agent.title.clone(),
        agent_session_id: None,
        agent_session_path: None,
        window: window.map(|window| window.address.clone()),
        workspace: window.and_then(|window| window.workspace.clone()),
        branch: None,
        focused: None,
        seen,
        herdr: Some(HerdrInfo {
            socket: session.socket.to_string_lossy().into_owned(),
            session: session.name.clone(),
            pane: agent.pane_id.clone(),
        }),
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
    use amon_protocol::AgentState;

    fn agent(status: &str) -> crate::herdr::wire::HerdrAgent {
        crate::herdr::wire::HerdrAgent {
            terminal_id: "t1".into(),
            agent: Some("claude".into()),
            agent_status: status.into(),
            pane_id: "w1:p2".into(),
            cwd: Some("/repo".into()),
            title: Some("fixing auth".into()),
            state_change_seq: Some(1),
        }
    }

    fn session() -> crate::herdr::proc::HerdrSession {
        crate::herdr::proc::HerdrSession {
            name: Some("work".into()),
            socket: "/tmp/herdr.sock".into(),
            client_pid: 42,
        }
    }

    #[test]
    fn herdr_statuses_map_onto_amon_states_and_seen() {
        // done is idle-and-unnoticed — exactly amon's finished-but-unseen.
        assert_eq!(state_for("done"), (AgentState::Idle, Some(false)));
        assert_eq!(state_for("idle"), (AgentState::Idle, Some(true)));
        assert_eq!(state_for("working"), (AgentState::Working, None));
        assert_eq!(state_for("blocked"), (AgentState::Blocked, Some(false)));
        assert_eq!(state_for("unknown"), (AgentState::Unknown, None));
        // A status this build has never heard of must not look urgent.
        assert_eq!(state_for("pondering"), (AgentState::Unknown, None));
    }

    #[test]
    fn an_entry_carries_identity_window_and_the_herdr_pointer() {
        let window = amon_hypr::Window {
            address: "abc123".into(),
            workspace: Some("3".into()),
        };
        let entry = entry_for(&agent("done"), &session(), Some(&window), 1000);
        assert!(entry.id.starts_with("herdr:"));
        assert!(entry.id.ends_with(":t1"));
        assert_eq!(entry.agent, "claude");
        assert_eq!(entry.state, AgentState::Idle);
        assert_eq!(entry.seen, Some(false));
        assert_eq!(entry.state_since, 1000);
        assert_eq!(entry.cwd, "/repo");
        assert_eq!(entry.title.as_deref(), Some("fixing auth"));
        assert_eq!(entry.window.as_deref(), Some("abc123"));
        assert_eq!(entry.workspace.as_deref(), Some("3"));
        assert!(!entry.hostname.is_empty());
        let herdr = entry.herdr.expect("projected entries carry provenance");
        assert_eq!(herdr.socket, "/tmp/herdr.sock");
        assert_eq!(herdr.session.as_deref(), Some("work"));
        assert_eq!(herdr.pane, "w1:p2");
    }

    #[test]
    fn an_agent_herdr_cannot_name_is_labelled_rather_than_blank() {
        let unnamed = crate::herdr::wire::HerdrAgent {
            agent: None,
            cwd: None,
            ..agent("blocked")
        };
        let entry = entry_for(&unnamed, &session(), None, 1);
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
        let entry = entry_for(&agent("idle"), &session, None, 1);
        assert!(entry.id.ends_with(":t1"));
        assert_eq!(entry.window, None);
        assert_eq!(entry.workspace, None);
        assert_eq!(entry.herdr.unwrap().session, None);
    }

    #[test]
    fn two_sessions_sharing_a_name_do_not_share_agent_ids() {
        // Two config homes, two sockets, the same session name — and herdr
        // hands out terminal ids that are only unique within the server that
        // issued them. An id built from the name would let one session's row
        // overwrite the other's in the panel, take the wrong row away on
        // disconnect, and send `focus --agent` to the wrong window.
        let mut one = session();
        one.socket = "/one/herdr.sock".into();
        let mut two = session();
        two.socket = "/two/herdr.sock".into();

        let left = entry_for(&agent("idle"), &one, None, 1);
        let right = entry_for(&agent("idle"), &two, None, 1);
        assert_ne!(left.id, right.id);
        // ...and the same session keeps the same id, or every resnapshot
        // would look like a new agent arriving.
        assert_eq!(left.id, entry_for(&agent("working"), &one, None, 2).id);
    }
}
