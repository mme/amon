//! luvus behind the seam.
//!
//! Where herdr subscribes status per pane, luvus has one global, sequenced
//! event stream: a single subscription covers every pane, and the last
//! `sequence` seen is handed back on reconnect so a blip loses nothing.
//! luvus also has no "agent appeared" event — a pane's agent label changing
//! is that moment — so any status event that does not describe an agent
//! already owned is structural, and answered with a fresh `agent.list`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use amon_protocol::Runtime;

use super::wire::{self, EventStream, RawEvent};
use super::{Hosted, HostedAgent, HostedEvent, Role, Session};

pub mod proc;

/// The UHP protocol number amon was written against. UHP promises additive
/// growth within a major; a different number is a different contract, and
/// projecting from it would be guessing.
const PROTOCOL: u64 = 1;

/// The codes luvus answers a cursor it will not honour with: older than its
/// replay window (`resync_required`), or newer than the sequence a
/// restarted server is at (`invalid_params` from `events.subscribe`;
/// `invalid_request` is the same refusal from `events.wait`, kept so a
/// future luvus aligning the two costs nothing).
const CURSOR_REFUSED: &[Option<&str>] = &[
    Some("resync_required"),
    Some("invalid_params"),
    Some("invalid_request"),
];

pub struct Luvus {
    /// The last event sequence seen, for the next subscription.
    pub(crate) after: Option<u64>,
}

/// The slice of luvus's `agent.list` record the projection needs.
#[derive(serde::Deserialize)]
struct Record {
    pane: String,
    status: String,
    #[serde(default)]
    agent: Option<String>,
    #[serde(default)]
    cwd: Option<String>,
}

/// The agents out of an `agent.list` result. Records missing a required
/// field are luvus's future, not our problem — skipped, never guessed at.
pub fn agents_from(result: &serde_json::Value) -> Vec<HostedAgent> {
    result
        .get("agents")
        .and_then(|agents| agents.as_array())
        .map(|agents| {
            agents
                .iter()
                .filter_map(|agent| serde_json::from_value::<Record>(agent.clone()).ok())
                .map(|record| HostedAgent {
                    identity: record.pane.clone(),
                    pane: record.pane,
                    agent: record.agent.filter(|agent| !agent.is_empty()),
                    status: record.status,
                    cwd: record.cwd,
                    life: None,
                })
                .collect()
        })
        .unwrap_or_default()
}

impl Hosted for Luvus {
    const KIND: &'static str = "luvus";
    const COMM: &'static str = "luvus";
    const STATUS_PER_PANE: bool = false;

    fn classify(argv: &[String], is_session_leader: bool, has_tty: bool, environ: &str) -> Role {
        proc::classify(argv, is_session_leader, has_tty, environ)
    }

    fn socket_for(argv: &[String], environ: &str) -> Option<PathBuf> {
        proc::socket_for(argv, environ)
    }

    fn new() -> Self {
        Luvus { after: None }
    }

    fn attach(
        &mut self,
        socket: &Path,
        _panes: &[String],
    ) -> std::io::Result<(Vec<HostedAgent>, EventStream)> {
        let pong = wire::request(socket, "ping", serde_json::json!({}))?;
        if pong.get("protocol").and_then(|p| p.as_u64()) != Some(PROTOCOL) {
            return Err(std::io::Error::other(format!(
                "luvus speaks protocol {}, not {PROTOCOL}",
                pong["protocol"]
            )));
        }
        let events = match self.after {
            // luvus refuses a cursor it cannot honour — older than its
            // replay window (`resync_required`), or newer than the sequence
            // a restarted server is at (`invalid_request`) — and either
            // refusal, retried with the same cursor forever, would leave the
            // rows frozen on the last state they had. The snapshot below
            // reads the present either way; the cursor was only ever about
            // the gap, and after a refusal there is no gap to close.
            Some(after) => {
                match wire::subscribe(socket, serde_json::json!({"after_sequence": after})) {
                    Ok(events) => events,
                    Err(error) if CURSOR_REFUSED.contains(&wire::error_code(&error).as_deref()) => {
                        self.after = None;
                        wire::subscribe(socket, serde_json::json!({}))?
                    }
                    // Anything else — the socket gone, a capacity refusal —
                    // is the outage the cursor exists to bridge, and clearing
                    // it here would lose exactly the events it will replay
                    // once the runtime is back.
                    Err(error) => return Err(error),
                }
            }
            None => wire::subscribe(socket, serde_json::json!({}))?,
        };
        // A subscription that opened without a cursor starts at the ack's
        // fence; a reconnect before any event arrives must still ask for
        // what that quiet stretch missed.
        if self.after.is_none() {
            self.after = events.fence();
        }
        let list = wire::request(socket, "agent.list", serde_json::json!({}))?;
        Ok((agents_from(&list), events))
    }

    fn event(&mut self, event: &RawEvent, owned: &HashMap<String, String>) -> HostedEvent {
        if let Some(sequence) = event.sequence {
            self.after = Some(sequence);
        }
        match event.event.as_str() {
            "pane.agent_status_changed" => {
                let (Some(pane), Some(status)) = (
                    event.data.get("pane").and_then(|pane| pane.as_str()),
                    event.data.get("status").and_then(|status| status.as_str()),
                ) else {
                    return HostedEvent::Ignore;
                };
                let agent = event
                    .data
                    .get("agent")
                    .and_then(|agent| agent.as_str())
                    .unwrap_or("");
                // A pane's agent label changing is an agent arriving or
                // leaving. For a pane amon holds, a different label — empty,
                // or another agent — means the one it held is gone, and the
                // snapshot that follows must not mistake a restart of the
                // same kind for the life that just ended. For a pane it does
                // not hold, a snapshot is the whole answer.
                match owned.get(pane) {
                    Some(label) if label == agent => HostedEvent::Status {
                        pane: pane.to_owned(),
                        status: status.to_owned(),
                        agent: Some(Some(agent.to_owned())),
                    },
                    Some(_) => HostedEvent::Departed {
                        pane: pane.to_owned(),
                    },
                    None => HostedEvent::Structural,
                }
            }
            "pane.closed" | "pane.created" | "pane.moved" | "events.resync_required" => {
                HostedEvent::Structural
            }
            _ => HostedEvent::Ignore,
        }
    }

    fn runtime(session: &Session, agent: &HostedAgent) -> Runtime {
        Runtime::Luvus {
            socket: session.socket.to_string_lossy().into_owned(),
            session: session.name.clone(),
            pane: agent.pane.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixListener;
    use std::sync::{Arc, Mutex};

    /// A canned luvus: answers each connection's first line with the next
    /// reply in order, records every request line, and holds subscriptions
    /// open.
    fn fake_luvus(
        replies: Vec<&'static str>,
    ) -> (tempfile::TempDir, PathBuf, Arc<Mutex<Vec<String>>>) {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("luvus.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let log = requests.clone();
        std::thread::spawn(move || {
            let mut replies = replies.into_iter();
            // Subscriptions are kept, not dropped: a closed stream would end
            // the subscription the attach is still holding.
            let mut held = Vec::new();
            for stream in listener.incoming().flatten() {
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap_or(0) == 0 {
                    continue;
                }
                log.lock().unwrap().push(line.clone());
                let mut stream = stream;
                if let Some(reply) = replies.next() {
                    let _ = writeln!(stream, "{reply}");
                }
                if line.contains("events.subscribe") {
                    held.push(stream);
                }
            }
        });
        (dir, socket, requests)
    }

    fn status_event(sequence: Option<u64>, data: serde_json::Value) -> RawEvent {
        RawEvent {
            event: "pane.agent_status_changed".into(),
            sequence,
            data,
        }
    }

    #[test]
    fn a_status_event_for_an_owned_pane_with_the_same_agent_is_a_status() {
        let mut host = Luvus::new();
        let owned = HashMap::from([("7".to_string(), "claude".to_string())]);
        let event = status_event(
            Some(41),
            serde_json::json!({"pane": "7", "status": "blocked", "agent": "claude"}),
        );
        assert_eq!(
            host.event(&event, &owned),
            HostedEvent::Status {
                pane: "7".into(),
                status: "blocked".into(),
                agent: Some(Some("claude".into())),
            }
        );
        assert_eq!(host.after, Some(41), "the cursor follows every event");
    }

    #[test]
    fn a_status_event_for_a_new_pane_is_structural_and_a_changed_agent_is_a_departure() {
        let mut host = Luvus::new();
        let owned = HashMap::from([("7".to_string(), "claude".to_string())]);
        let unknown = status_event(
            Some(1),
            serde_json::json!({"pane": "8", "status": "working", "agent": "codex"}),
        );
        assert_eq!(host.event(&unknown, &owned), HostedEvent::Structural);
        let exited = status_event(
            Some(2),
            serde_json::json!({"pane": "7", "status": "idle", "agent": ""}),
        );
        assert_eq!(
            host.event(&exited, &owned),
            HostedEvent::Departed { pane: "7".into() }
        );
        let replaced = status_event(
            Some(3),
            serde_json::json!({"pane": "7", "status": "working", "agent": "codex"}),
        );
        assert_eq!(
            host.event(&replaced, &owned),
            HostedEvent::Departed { pane: "7".into() }
        );
    }

    #[test]
    fn pane_lifecycle_and_resync_are_structural_and_the_rest_is_noise() {
        let mut host = Luvus::new();
        let owned = HashMap::new();
        for name in [
            "pane.closed",
            "pane.created",
            "pane.moved",
            "events.resync_required",
        ] {
            let event = RawEvent {
                event: name.into(),
                sequence: None,
                data: serde_json::json!({}),
            };
            assert_eq!(
                host.event(&event, &owned),
                HostedEvent::Structural,
                "{name}"
            );
        }
        let frame = RawEvent {
            event: "terminal.frame".into(),
            sequence: None,
            data: serde_json::json!({}),
        };
        assert_eq!(host.event(&frame, &owned), HostedEvent::Ignore);
    }

    #[test]
    fn agent_list_records_become_hosted_agents_keyed_by_pane() {
        let result = serde_json::json!({"type": "agent_list", "agents": [
            {"pane": "7", "agent": "claude", "name": null, "status": "done", "cwd": "/repo"},
            {"pane": "9", "agent": "codex"}
        ]});
        let agents = agents_from(&result);
        assert_eq!(agents.len(), 1, "no status: skipped, never guessed");
        assert_eq!(
            agents[0],
            HostedAgent {
                identity: "7".into(),
                pane: "7".into(),
                agent: Some("claude".into()),
                status: "done".into(),
                cwd: Some("/repo".into()),
                life: None,
            }
        );
    }

    #[test]
    fn attach_pings_for_protocol_one_subscribes_after_the_cursor_and_lists_agents() {
        let (_dir, socket, requests) = fake_luvus(vec![
            r#"{"id":"amon","result":{"type":"pong","version":"0.13.2","protocol":1}}"#,
            r#"{"id":"amon-sub","result":{"type":"subscription_started","sequence":41}}"#,
            r#"{"id":"amon","result":{"type":"agent_list","agents":[]}}"#,
        ]);
        let mut host = Luvus { after: Some(40) };
        let (agents, _events) = host.attach(&socket, &[]).unwrap();
        assert!(agents.is_empty());
        let seen = requests.lock().unwrap().join("");
        assert!(seen.contains(r#""method":"ping""#), "{seen}");
        assert!(seen.contains(r#""after_sequence":40"#), "{seen}");
        assert!(seen.contains(r#""method":"agent.list""#), "{seen}");
    }

    #[test]
    fn a_refused_cursor_is_dropped_and_the_subscription_retried_without_it() {
        // luvus's exact answers: a cursor older than the replay window, and
        // — after a server restart — one newer than its current sequence.
        for refusal in [
            r#"{"id":"amon-sub","error":{"code":"resync_required","message":"requested event history is no longer retained","sequence":900}}"#,
            r#"{"id":"amon-sub","error":{"code":"invalid_params","message":"after_sequence is newer than the current event sequence","sequence":2}}"#,
        ] {
            let (_dir, socket, requests) = fake_luvus(vec![
                r#"{"id":"amon","result":{"type":"pong","version":"0.13.2","protocol":1}}"#,
                refusal,
                r#"{"id":"amon-sub","result":{"type":"subscription_started","sequence":900}}"#,
                r#"{"id":"amon","result":{"type":"agent_list","agents":[]}}"#,
            ]);
            let mut host = Luvus { after: Some(3) };
            host.attach(&socket, &[]).unwrap();
            assert_eq!(host.after, Some(900), "the retry starts at the ack's fence");
            let seen = requests.lock().unwrap().clone();
            assert_eq!(seen.len(), 4, "{refusal}");
            assert!(seen[1].contains(r#""after_sequence":3"#), "{}", seen[1]);
            assert!(
                seen[2].contains("events.subscribe") && !seen[2].contains("after_sequence"),
                "{}",
                seen[2]
            );
            assert!(seen[3].contains("agent.list"));
        }
    }

    #[test]
    fn a_quiet_subscription_still_leaves_a_cursor_for_the_next_attach() {
        let (_dir, socket, requests) = fake_luvus(vec![
            r#"{"id":"amon","result":{"type":"pong","version":"0.13.2","protocol":1}}"#,
            r#"{"id":"amon-sub","result":{"type":"subscription_started","sequence":41}}"#,
            r#"{"id":"amon","result":{"type":"agent_list","agents":[]}}"#,
            r#"{"id":"amon","result":{"type":"pong","version":"0.13.2","protocol":1}}"#,
            r#"{"id":"amon-sub","result":{"type":"subscription_started","sequence":41}}"#,
            r#"{"id":"amon","result":{"type":"agent_list","agents":[]}}"#,
        ]);
        let mut host = Luvus::new();
        host.attach(&socket, &[]).unwrap();
        assert_eq!(host.after, Some(41));
        host.attach(&socket, &[]).unwrap();
        let seen = requests.lock().unwrap().clone();
        assert!(seen[4].contains(r#""after_sequence":41"#), "{}", seen[4]);
    }

    #[test]
    fn any_other_subscription_failure_keeps_the_cursor_for_the_replay() {
        // A full subscriber table, or no socket at all, is the outage the
        // cursor exists to bridge: the next attach must still ask for what
        // was missed.
        let (_dir, socket, requests) = fake_luvus(vec![
            r#"{"id":"amon","result":{"type":"pong","version":"0.13.2","protocol":1}}"#,
            r#"{"id":"amon-sub","error":{"code":"unavailable","message":"event subscriber capacity is full"}}"#,
        ]);
        let mut host = Luvus { after: Some(3) };
        assert!(host.attach(&socket, &[]).is_err());
        assert_eq!(host.after, Some(3));
        assert_eq!(requests.lock().unwrap().len(), 2, "no uncursored retry");

        let mut host = Luvus { after: Some(3) };
        assert!(host
            .attach(Path::new("/nonexistent/luvus.sock"), &[])
            .is_err());
        assert_eq!(host.after, Some(3));
    }

    #[test]
    fn a_foreign_protocol_is_refused_before_anything_is_projected() {
        let (_dir, socket, requests) = fake_luvus(vec![
            r#"{"id":"amon","result":{"type":"pong","version":"9.0.0","protocol":2}}"#,
        ]);
        assert!(Luvus::new().attach(&socket, &[]).is_err());
        assert_eq!(requests.lock().unwrap().len(), 1, "nothing past the ping");
    }
}
