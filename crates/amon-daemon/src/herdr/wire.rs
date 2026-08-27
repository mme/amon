//! Speaking herdr's socket: newline-delimited JSON, one request per
//! connection, except a subscription, which acks and then streams.
//!
//! Deliberately raw the whole way down — the daemon never shells out to the
//! `herdr` CLI, whose protocol guard hard-fails on any version mismatch,
//! while the socket serves any client and grows additively. Parsing is
//! permissive for the same reason: unknown fields are herdr's future, not
//! errors.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::Shutdown;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::Duration;

const IO_TIMEOUT: Duration = Duration::from_secs(5);
/// Bounds what a peer can make us hold. Far above any real response —
/// herdr's own request cap is 1 MiB — and a cut line fails to parse rather
/// than growing the daemon.
const MAX_LINE: u64 = 8 * 1024 * 1024;

/// One pushed event line: `{"event":"...","data":{...}}`.
pub struct HerdrEvent {
    pub event: String,
    pub data: serde_json::Value,
}

/// The slice of herdr's agent record the projection needs. Everything else
/// on the wire is ignored, which is what keeps this compatible across herdr
/// releases.
///
/// Only what herdr's schema marks required is required here. `agent`, `cwd`
/// and `title` are nullable *and* optional there, and `Option` is what
/// accepts both spellings — a bare `String` with `#[serde(default)]` takes a
/// missing field but fails on an explicit `null`, and a failed record is
/// dropped by [`agents_from`], which is how a live agent would quietly
/// vanish from the bar.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
pub struct HerdrAgent {
    pub terminal_id: String,
    pub agent_status: String,
    pub pane_id: String,
    #[serde(default)]
    pub agent: Option<String>,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    /// herdr's per-agent transition counter. It counts up through one
    /// agent's life, so a value that went *backwards* is a different agent
    /// in the same terminal — which is the only way to tell one claude from
    /// the next one started in the pane it just left.
    #[serde(default)]
    pub state_change_seq: Option<u64>,
}

/// One request, one line back: connect, send, read the `result` value.
pub fn request(
    socket: &Path,
    method: &str,
    params: serde_json::Value,
) -> std::io::Result<serde_json::Value> {
    let stream = UnixStream::connect(socket)?;
    stream.set_read_timeout(Some(IO_TIMEOUT))?;
    stream.set_write_timeout(Some(IO_TIMEOUT))?;
    let mut writer = stream.try_clone()?;
    let line = serde_json::json!({"id": "amon", "method": method, "params": params});
    writeln!(writer, "{line}")?;
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader.by_ref().take(MAX_LINE).read_line(&mut line)?;
    parse_result(&line)
}

fn parse_result(line: &str) -> std::io::Result<serde_json::Value> {
    let response: serde_json::Value = serde_json::from_str(line)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
    if let Some(error) = response.get("error") {
        return Err(std::io::Error::other(error.to_string()));
    }
    response
        .get("result")
        .cloned()
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidData, "no result"))
}

/// A live `events.subscribe` connection, iterating pushed events.
pub struct EventStream {
    reader: BufReader<UnixStream>,
    stream: UnixStream,
}

/// Ends an [`EventStream`]'s blocking read from another thread — the reader
/// itself cannot poll a flag while parked in `read_line`.
pub struct ShutdownHandle(UnixStream);

impl ShutdownHandle {
    pub fn shutdown(&self) {
        let _ = self.0.shutdown(Shutdown::Both);
    }
}

/// Opens a subscription. The set is fixed for the connection's lifetime —
/// herdr has no way to add to it — so changing it means a new call.
pub fn subscribe(
    socket: &Path,
    subscriptions: Vec<serde_json::Value>,
) -> std::io::Result<EventStream> {
    let stream = UnixStream::connect(socket)?;
    stream.set_write_timeout(Some(IO_TIMEOUT))?;
    let mut writer = stream.try_clone()?;
    let line = serde_json::json!({
        "id": "amon-sub",
        "method": "events.subscribe",
        "params": {"subscriptions": subscriptions},
    });
    writeln!(writer, "{line}")?;
    let mut reader = BufReader::new(stream.try_clone()?);
    // The ack has a deadline; the events after it do not — the stream then
    // blocks for as long as nothing happens, and teardown is the handle's
    // job.
    stream.set_read_timeout(Some(IO_TIMEOUT))?;
    let mut ack = String::new();
    reader.by_ref().take(MAX_LINE).read_line(&mut ack)?;
    parse_result(&ack)?;
    stream.set_read_timeout(None)?;
    Ok(EventStream { reader, stream })
}

impl EventStream {
    pub fn shutdown_handle(&self) -> ShutdownHandle {
        ShutdownHandle(
            self.stream
                .try_clone()
                .expect("cloning a connected unix stream"),
        )
    }
}

impl Iterator for EventStream {
    type Item = HerdrEvent;

    /// The next event, skipping lines that are not one. `None` is the end of
    /// the stream, however it ended — the caller's answer to that is a fresh
    /// connection, not a diagnosis.
    fn next(&mut self) -> Option<HerdrEvent> {
        loop {
            let mut line = String::new();
            match self.reader.by_ref().take(MAX_LINE).read_line(&mut line) {
                Ok(0) | Err(_) => return None,
                Ok(_) => {}
            }
            let Ok(value) = serde_json::from_str::<serde_json::Value>(&line) else {
                continue;
            };
            let Some(event) = value.get("event").and_then(|event| event.as_str()) else {
                continue;
            };
            return Some(HerdrEvent {
                event: event.to_owned(),
                data: value
                    .get("data")
                    .cloned()
                    .unwrap_or(serde_json::Value::Null),
            });
        }
    }
}

/// The agents out of a `session.snapshot` result. herdr 0.8.0 nests the
/// payload under `snapshot`; the docs draw it flat — read both, prefer what
/// the live server actually sends. Records missing a required field are
/// herdr's future, not our problem — skipped, never guessed at.
pub fn agents_from(result: &serde_json::Value) -> Vec<HerdrAgent> {
    result
        .get("snapshot")
        .and_then(|snapshot| snapshot.get("agents"))
        .or_else(|| result.get("agents"))
        .and_then(|agents| agents.as_array())
        .map(|agents| {
            agents
                .iter()
                .filter_map(|agent| serde_json::from_value(agent.clone()).ok())
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixListener;

    /// One canned herdr: answers the first line per `replies`, then streams
    /// `pushes`, then holds the connection open until dropped.
    fn fake_herdr(
        replies: impl Fn(&str) -> String + Send + 'static,
        pushes: Vec<String>,
    ) -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("herdr.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap_or(0) == 0 {
                    continue;
                }
                let mut stream = stream;
                let _ = writeln!(stream, "{}", replies(line.trim_end()));
                for push in &pushes {
                    let _ = writeln!(stream, "{push}");
                }
                // Held open: subscriptions outlive their pushes.
                std::thread::sleep(std::time::Duration::from_secs(5));
            }
        });
        (dir, socket)
    }

    #[test]
    fn a_request_returns_the_result_value() {
        let (_dir, socket) = fake_herdr(
            |_| r#"{"id":"r","result":{"type":"pong","protocol":21}}"#.into(),
            vec![],
        );
        let result = request(&socket, "ping", serde_json::json!({})).unwrap();
        assert_eq!(result["protocol"], 21);
    }

    #[test]
    fn an_error_response_is_an_error() {
        let (_dir, socket) = fake_herdr(
            |_| r#"{"id":"r","error":{"code":"not_found","message":"no"}}"#.into(),
            vec![],
        );
        assert!(request(&socket, "pane.get", serde_json::json!({})).is_err());
    }

    #[test]
    fn a_request_names_its_method_and_params_on_the_wire() {
        let (_dir, socket) = fake_herdr(
            |line| {
                let request: serde_json::Value = serde_json::from_str(line).unwrap();
                assert_eq!(request["method"], "agent.focus");
                assert_eq!(request["params"]["target"], "w1:p1");
                assert!(request["id"].is_string());
                r#"{"id":"r","result":{"type":"agent_focus"}}"#.into()
            },
            vec![],
        );
        request(
            &socket,
            "agent.focus",
            serde_json::json!({"target": "w1:p1"}),
        )
        .unwrap();
    }

    #[test]
    fn a_subscription_acks_then_streams_events() {
        let (_dir, socket) = fake_herdr(
            |line| {
                assert!(line.contains("events.subscribe"));
                r#"{"id":"s","result":{"type":"subscription_started"}}"#.into()
            },
            vec![
                r#"{"event":"pane.agent_status_changed","data":{"pane_id":"w1:p1","agent_status":"blocked"}}"#.into(),
            ],
        );
        let mut events =
            subscribe(&socket, vec![serde_json::json!({"type": "pane.closed"})]).unwrap();
        let event = events.next().unwrap();
        assert_eq!(event.event, "pane.agent_status_changed");
        assert_eq!(event.data["agent_status"], "blocked");
    }

    #[test]
    fn shutdown_unblocks_a_waiting_reader() {
        let (_dir, socket) = fake_herdr(
            |_| r#"{"id":"s","result":{"type":"subscription_started"}}"#.into(),
            vec![],
        );
        let mut events = subscribe(&socket, vec![]).unwrap();
        let handle = events.shutdown_handle();
        let reader = std::thread::spawn(move || events.next());
        handle.shutdown();
        assert!(reader.join().unwrap().is_none());
    }

    #[test]
    fn snapshot_agents_are_extracted_and_partial_records_skipped() {
        // The shape herdr 0.8.0 actually sends: the payload nested under
        // "snapshot" (verified against a live server).
        let snapshot: serde_json::Value = serde_json::json!({
            "type": "session_snapshot",
            "snapshot": {
                "version": "0.8.0",
                "agents": [
                    {"terminal_id":"t1","agent":"claude","agent_status":"working",
                     "pane_id":"w1:p1","cwd":"/repo","title":"fixing"},
                    {"agent":"mystery"}
                ]
            }
        });
        let agents = agents_from(&snapshot);
        assert_eq!(agents.len(), 1);
        assert_eq!(agents[0].terminal_id, "t1");
        assert_eq!(agents[0].agent_status, "working");
        assert_eq!(agents[0].pane_id, "w1:p1");
        assert_eq!(agents[0].title.as_deref(), Some("fixing"));
    }

    #[test]
    fn an_agent_survives_the_fields_herdr_is_allowed_to_leave_out() {
        // herdr's schema requires only terminal_id, pane_id and
        // agent_status; `agent`, `cwd` and `title` are nullable and
        // optional. A null there must not delete the agent from amon —
        // silently dropping the record is how a live agent vanishes from
        // the bar for a reason nobody can see.
        let snapshot = serde_json::json!({
            "agents": [
                {"terminal_id":"t1","pane_id":"w1:p1","agent_status":"blocked",
                 "agent": null, "cwd": null, "title": null},
                {"terminal_id":"t2","pane_id":"w1:p2","agent_status":"idle"},
            ]
        });
        let agents = agents_from(&snapshot);
        assert_eq!(agents.len(), 2, "nullable fields are not required fields");
        assert_eq!(agents[0].agent, None);
        assert_eq!(agents[0].cwd, None);
        assert_eq!(agents[1].terminal_id, "t2");
    }

    #[test]
    fn a_record_missing_what_herdr_always_sends_is_still_skipped() {
        let snapshot = serde_json::json!({"agents": [{"agent": "mystery"}]});
        assert!(agents_from(&snapshot).is_empty());
    }

    #[test]
    fn a_snapshot_with_agents_at_the_top_level_reads_the_same() {
        // The 0.8.2 docs draw the response flat; accept both spellings so a
        // future herdr un-nesting it costs nothing.
        let snapshot: serde_json::Value = serde_json::json!({
            "type": "session_snapshot",
            "agents": [
                {"terminal_id":"t1","agent":"claude","agent_status":"idle",
                 "pane_id":"w1:p1"}
            ]
        });
        assert_eq!(agents_from(&snapshot).len(), 1);
    }
}
