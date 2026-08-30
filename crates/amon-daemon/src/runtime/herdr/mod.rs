//! herdr behind the seam.
//!
//! herdr has no global status subscription — `pane.agent_status_changed` is
//! per pane, and a connection's set is fixed — which is why the supervisor
//! re-attaches whenever the set of agent panes changes, and why every
//! structural event (an agent appearing or leaving, a pane closing or
//! moving) is answered with a fresh snapshot rather than bookkeeping.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use amon_protocol::Runtime;

use super::wire::{self, EventStream, RawEvent};
use super::{Hosted, HostedAgent, HostedEvent, Role, Session};

pub mod agents;
pub mod proc;

pub struct Herdr;

/// Global lifecycle events, plus one status subscription per known agent
/// pane.
fn subscriptions_for<'a>(panes: impl Iterator<Item = &'a str>) -> Vec<serde_json::Value> {
    let mut subscriptions = vec![
        serde_json::json!({"type": "pane.agent_detected"}),
        serde_json::json!({"type": "pane.closed"}),
        serde_json::json!({"type": "pane.exited"}),
        serde_json::json!({"type": "pane.moved"}),
    ];
    subscriptions.extend(
        panes.map(|pane| serde_json::json!({"type": "pane.agent_status_changed", "pane_id": pane})),
    );
    subscriptions
}

impl Hosted for Herdr {
    const KIND: &'static str = "herdr";
    const COMM: &'static str = "herdr";
    const STATUS_PER_PANE: bool = true;

    fn classify(argv: &[String], is_session_leader: bool, has_tty: bool, environ: &str) -> Role {
        proc::classify(argv, is_session_leader, has_tty, environ)
    }

    fn socket_for(argv: &[String], environ: &str) -> Option<PathBuf> {
        proc::socket_for(argv, environ)
    }

    fn new() -> Self {
        Herdr
    }

    fn attach(
        &mut self,
        socket: &Path,
        panes: &[String],
    ) -> std::io::Result<(Vec<HostedAgent>, EventStream)> {
        let subscriptions = subscriptions_for(panes.iter().map(String::as_str));
        let events = wire::subscribe(socket, serde_json::json!({"subscriptions": subscriptions}))?;
        let snapshot = wire::request(socket, "session.snapshot", serde_json::json!({}))?;
        let agents = agents::agents_from(&snapshot)
            .into_iter()
            .map(Into::into)
            .collect();
        Ok((agents, events))
    }

    fn event(&mut self, event: &RawEvent, _owned: &HashMap<String, String>) -> HostedEvent {
        if event.event != "pane.agent_status_changed" {
            // Structural: start over with a fresh snapshot.
            return HostedEvent::Structural;
        }
        let (Some(pane), Some(status)) = (
            event.data.get("pane_id").and_then(|id| id.as_str()),
            event
                .data
                .get("agent_status")
                .and_then(|status| status.as_str()),
        ) else {
            return HostedEvent::Ignore;
        };
        // Present-and-null means herdr no longer has a name for what is in
        // the pane; absent means it did not say. The projection reads a null
        // the same way, and the two paths must agree or the row keeps the
        // name of an agent that has gone.
        let agent = match event.data.get("agent") {
            None => None,
            Some(serde_json::Value::Null) => Some(None),
            Some(agent) => agent.as_str().map(|agent| Some(agent.to_owned())),
        };
        HostedEvent::Status {
            pane: pane.to_owned(),
            status: status.to_owned(),
            agent,
        }
    }

    fn runtime(session: &Session, agent: &HostedAgent) -> Runtime {
        Runtime::Herdr {
            socket: session.socket.to_string_lossy().into_owned(),
            session: session.name.clone(),
            pane: agent.pane.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn herdr_events_split_into_status_and_structural() {
        let mut host = Herdr::new();
        let owned = HashMap::new();
        let status = RawEvent {
            event: "pane.agent_status_changed".into(),
            sequence: None,
            data: serde_json::json!({"pane_id": "w1:p1", "agent_status": "blocked", "agent": null}),
        };
        assert_eq!(
            host.event(&status, &owned),
            HostedEvent::Status {
                pane: "w1:p1".into(),
                status: "blocked".into(),
                agent: Some(None),
            }
        );
        let closed = RawEvent {
            event: "pane.closed".into(),
            sequence: None,
            data: serde_json::json!({}),
        };
        assert_eq!(host.event(&closed, &owned), HostedEvent::Structural);
    }

    #[test]
    fn the_subscription_covers_lifecycle_globally_and_status_per_pane() {
        let subscriptions = subscriptions_for(["w1:p1", "w2:p3"].into_iter());
        assert_eq!(subscriptions.len(), 6);
        assert_eq!(subscriptions[4]["type"], "pane.agent_status_changed");
        assert_eq!(subscriptions[4]["pane_id"], "w1:p1");
        assert_eq!(subscriptions[5]["pane_id"], "w2:p3");
    }
}
