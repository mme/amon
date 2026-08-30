//! Speaking a runtime's socket: newline-delimited JSON, one request per
//! connection, except a subscription, which acks and then streams. herdr
//! and luvus share the framing to the byte; only method names differ.
//!
//! Deliberately raw the whole way down — the daemon never shells out to a
//! runtime's CLI, whose protocol guard may hard-fail on any version
//! mismatch, while the socket serves any client and grows additively.
//! Parsing is permissive for the same reason: unknown fields are the
//! runtime's future, not errors.

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

/// One pushed event line: `{"event":"...","data":{...}}`, with a
/// `sequence` where the runtime numbers its events.
#[derive(Debug, Clone, PartialEq)]
pub struct RawEvent {
    pub event: String,
    pub sequence: Option<u64>,
    pub data: serde_json::Value,
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

/// Opens a subscription with the given `params`. What the params mean is the
/// runtime's business: herdr takes a fixed list of subscriptions, luvus a
/// cursor. Either way the set is fixed for the connection's lifetime, so
/// changing it means a new call.
pub fn subscribe(socket: &Path, params: serde_json::Value) -> std::io::Result<EventStream> {
    let stream = UnixStream::connect(socket)?;
    stream.set_write_timeout(Some(IO_TIMEOUT))?;
    let mut writer = stream.try_clone()?;
    let line = serde_json::json!({
        "id": "amon-sub",
        "method": "events.subscribe",
        "params": params,
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
    type Item = RawEvent;

    /// The next event, skipping lines that are not one. `None` is the end of
    /// the stream, however it ended — the caller's answer to that is a fresh
    /// connection, not a diagnosis.
    fn next(&mut self) -> Option<RawEvent> {
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
            return Some(RawEvent {
                event: event.to_owned(),
                sequence: value.get("sequence").and_then(|seq| seq.as_u64()),
                data: value
                    .get("data")
                    .cloned()
                    .unwrap_or(serde_json::Value::Null),
            });
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixListener;

    /// One canned runtime: answers the first line per `replies`, then streams
    /// `pushes`, then holds the connection open until dropped.
    pub(crate) fn fake_runtime(
        replies: impl Fn(&str) -> String + Send + 'static,
        pushes: Vec<String>,
    ) -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("runtime.sock");
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
        let (_dir, socket) = fake_runtime(
            |_| r#"{"id":"r","result":{"type":"pong","protocol":21}}"#.into(),
            vec![],
        );
        let result = request(&socket, "ping", serde_json::json!({})).unwrap();
        assert_eq!(result["protocol"], 21);
    }

    #[test]
    fn an_error_response_is_an_error() {
        let (_dir, socket) = fake_runtime(
            |_| r#"{"id":"r","error":{"code":"not_found","message":"no"}}"#.into(),
            vec![],
        );
        assert!(request(&socket, "pane.get", serde_json::json!({})).is_err());
    }

    #[test]
    fn a_request_names_its_method_and_params_on_the_wire() {
        let (_dir, socket) = fake_runtime(
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
    fn a_subscription_acks_then_streams_events_with_their_sequence() {
        let (_dir, socket) = fake_runtime(
            |line| {
                assert!(line.contains("events.subscribe"));
                assert!(line.contains(r#""after_sequence":3"#));
                r#"{"id":"s","result":{"type":"subscription_started"}}"#.into()
            },
            vec![
                r#"{"event":"pane.agent_status_changed","sequence":4,"data":{"pane":"7","status":"blocked"}}"#.into(),
                r#"{"event":"pane.closed","data":{"pane_id":"w1:p1"}}"#.into(),
            ],
        );
        let mut events = subscribe(&socket, serde_json::json!({"after_sequence": 3})).unwrap();
        let event = events.next().unwrap();
        assert_eq!(event.event, "pane.agent_status_changed");
        assert_eq!(event.sequence, Some(4));
        assert_eq!(event.data["status"], "blocked");
        let event = events.next().unwrap();
        assert_eq!(event.sequence, None, "herdr numbers nothing");
    }

    #[test]
    fn shutdown_unblocks_a_waiting_reader() {
        let (_dir, socket) = fake_runtime(
            |_| r#"{"id":"s","result":{"type":"subscription_started"}}"#.into(),
            vec![],
        );
        let mut events = subscribe(&socket, serde_json::json!({})).unwrap();
        let handle = events.shutdown_handle();
        let reader = std::thread::spawn(move || events.next());
        handle.shutdown();
        assert!(reader.join().unwrap().is_none());
    }
}
