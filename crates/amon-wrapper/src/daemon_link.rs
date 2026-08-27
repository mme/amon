//! The wrapper's best-effort connection to the daemon.
//!
//! Everything here is allowed to fail. Observability is not worth interrupting
//! someone's coding session, so a daemon that is missing, wedged, out of date,
//! or speaking nonsense must cost the agent nothing: no panic, no blocking, no
//! output. The link runs on its own thread and the wrapper never waits on it.
//!
//! The link holds the agent's current entry rather than a queue of pending
//! messages. Patches mutate that entry; a reconnect re-registers it. So a
//! daemon restart costs one registration, never a replay of stale history, and
//! a long disconnection cannot grow unbounded (ADR-0002).

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::Duration;

use amon_protocol::{AgentEntry, AgentPatch, Hello, Method, Request, Role, PROTOCOL_VERSION};

use crate::whisper;

const FIRST_RETRY: Duration = Duration::from_millis(100);
const MAX_RETRY: Duration = Duration::from_secs(30);
/// How often a connected link proves itself.
///
/// A socket is only tested by writing to it, so a wrapper with nothing to say
/// never learns that the daemon went away. It waits for the agent's next state
/// change — which for an idle agent can be hours — and until then it is
/// connected to nothing while believing otherwise. Restarting the daemon left
/// two of three live agents invisible, and they were invisible because they
/// were quiet.
///
/// Re-registering rather than pinging, because the wrapper keeps its entry
/// current by applying its own patches to it: the message that proves the
/// socket is alive is exactly the one that restores this agent to a daemon that
/// came back empty. Detection and repair are the same act.
///
/// The registry answers a repeat register on the same connection with
/// `AgentUpdated` rather than `AgentConnected`, so no subscriber sees a
/// reconnection that did not happen.
/// Five seconds: long enough that the traffic is nothing — one small message
/// per agent, and the registry answers it without waking a subscriber that
/// would not otherwise have been woken — and short enough that an agent lost to
/// a daemon restart is back before anyone goes looking for it.
const HEARTBEAT: Duration = Duration::from_secs(5);
/// Long enough that a wedged daemon cannot stall the link thread behind the
/// agent's own progress, short enough to notice a dead socket promptly.
const IO_TIMEOUT: Duration = Duration::from_secs(2);

pub enum Message {
    Register(Box<AgentEntry>),
    Update(Box<AgentPatch>),
    /// A knock's answer arrived on the input stream: whispering may begin.
    WhisperArmed,
}

/// The whisper tee: the link's own traffic, mirrored into the outbox once a
/// listener has proven itself. Before that — and always, when the wrapper
/// was built without an outbox — every call is silence.
///
/// The tee sits beside the daemon connection, not behind it: a wrapper whose
/// own daemon is missing still whispers, because the listener that matters is
/// on the far side of the terminal stream.
struct Tee {
    outbox: Option<whisper::Outbox>,
}

impl Tee {
    fn event(&self, method: Method) {
        if let Some(outbox) = self.outbox.as_ref().filter(|outbox| outbox.armed()) {
            outbox.enqueue(whisper::encode(&whisper::WhisperFrame::Event(Box::new(
                method,
            ))));
        }
    }

    /// The answer arrived. Everything said before it is gone, so the held
    /// entry goes out first, as a snapshot the far side can anchor to.
    fn armed(&self, entry: Option<&AgentEntry>) {
        let Some(outbox) = self.outbox.as_ref() else {
            return;
        };
        outbox.arm();
        if let Some(entry) = entry {
            self.event(Method::AgentRegister(entry.clone()));
        }
    }

    /// A quiet wake: the whispered heartbeat, mirroring the link's own
    /// re-register-as-heartbeat. Wakes are at most 30 seconds apart, inside
    /// the far side's revert window.
    fn heartbeat(&self, entry: Option<&AgentEntry>) {
        if let Some(entry) = entry {
            self.event(Method::AgentRegister(entry.clone()));
        }
    }
}

/// Handle held by the rest of the wrapper. Sending never blocks and never
/// fails in a way the caller must handle: if the link thread is gone, the
/// message is dropped.
#[derive(Clone)]
pub struct DaemonLink {
    tx: mpsc::Sender<Message>,
}

impl DaemonLink {
    /// `whisper`: where to mirror this link's traffic for a listener on the
    /// far side of the terminal stream — `None` outside an ssh session.
    pub fn spawn(version: String, whisper: Option<whisper::Outbox>) -> Self {
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || run(rx, version, Tee { outbox: whisper }));
        Self { tx }
    }

    pub fn whisper_armed(&self) {
        let _ = self.tx.send(Message::WhisperArmed);
    }

    /// A link whose far end is the test: no thread, no socket, every message
    /// readable from the returned receiver.
    #[cfg(test)]
    pub(crate) fn test() -> (Self, mpsc::Receiver<Message>) {
        let (tx, rx) = mpsc::channel();
        (Self { tx }, rx)
    }

    pub fn register(&self, entry: AgentEntry) {
        let _ = self.tx.send(Message::Register(Box::new(entry)));
    }

    pub fn update(&self, patch: AgentPatch) {
        let _ = self.tx.send(Message::Update(Box::new(patch)));
    }
}

fn run(rx: mpsc::Receiver<Message>, version: String, tee: Tee) {
    let mut entry: Option<AgentEntry> = None;
    let mut connection: Option<Connection> = None;
    let mut retry = FIRST_RETRY;

    loop {
        // Connected, wake on the heartbeat; disconnected, wake to retry.
        let wait = if connection.is_some() {
            HEARTBEAT
        } else {
            retry
        };

        match rx.recv_timeout(wait) {
            Ok(Message::Register(new_entry)) => {
                entry = Some(*new_entry);
                tee.heartbeat(entry.as_ref());
                if let Some(link) = connection.as_mut() {
                    if let Some(entry) = entry.as_ref() {
                        if link.send(Method::AgentRegister(entry.clone())).is_err() {
                            connection = None;
                        }
                    }
                }
            }
            Ok(Message::Update(patch)) => {
                let changed = entry
                    .as_mut()
                    .map(|entry| patch.apply(entry))
                    .unwrap_or(false);
                if changed {
                    tee.event(Method::AgentUpdate((*patch).clone()));
                    if let Some(link) = connection.as_mut() {
                        if link.send(Method::AgentUpdate(*patch)).is_err() {
                            connection = None;
                        }
                    }
                }
            }
            Ok(Message::WhisperArmed) => {
                tee.armed(entry.as_ref());
            }
            Err(RecvTimeoutError::Timeout) => {
                tee.heartbeat(entry.as_ref());
                // Nothing to say. While connected that is the heartbeat: send
                // the entry we hold, which either goes through — proving the
                // daemon is there and restoring us if it had forgotten — or
                // fails, and the reconnect below takes over.
                if let (Some(link), Some(entry)) = (connection.as_mut(), entry.as_ref()) {
                    if link.send(Method::AgentRegister(entry.clone())).is_err() {
                        connection = None;
                    }
                }
            }
            // The wrapper is shutting down. Dropping the connection is how the
            // daemon learns the agent is gone (ADR-0002), so there is nothing
            // to send.
            Err(RecvTimeoutError::Disconnected) => return,
        }

        if connection.is_none() {
            if let Some(entry) = entry.as_ref() {
                match connect(&version) {
                    Some(mut link) => {
                        if link.send(Method::AgentRegister(entry.clone())).is_ok() {
                            connection = Some(link);
                            retry = FIRST_RETRY;
                        } else {
                            retry = (retry * 2).min(MAX_RETRY);
                        }
                    }
                    None => retry = (retry * 2).min(MAX_RETRY),
                }
            }
        }
    }
}

/// Connects, starting a daemon if none is listening. The connect-or-spawn
/// primitive is shared with the CLI; the caller's backoff covers a daemon
/// that never comes up.
fn connect(version: &str) -> Option<Connection> {
    let stream = amon_protocol::connect_or_spawn_daemon()?;
    Connection::open(stream, version)
}

struct Connection {
    stream: UnixStream,
    reader: BufReader<UnixStream>,
    next_id: u64,
}

impl Connection {
    fn open(stream: UnixStream, version: &str) -> Option<Self> {
        stream.set_read_timeout(Some(IO_TIMEOUT)).ok()?;
        stream.set_write_timeout(Some(IO_TIMEOUT)).ok()?;
        let reader = BufReader::new(stream.try_clone().ok()?);
        let mut link = Self {
            stream,
            reader,
            next_id: 0,
        };
        link.send(Method::Hello(Hello {
            role: Role::Wrapper,
            protocol: PROTOCOL_VERSION,
            version: version.to_string(),
        }))
        .ok()?;
        Some(link)
    }

    /// Sends one request and waits for its response. The response is discarded
    /// — an error reply from a newer daemon that dislikes something we sent is
    /// not worth acting on, only surviving.
    fn send(&mut self, method: Method) -> std::io::Result<()> {
        self.next_id += 1;
        let request = Request::new(format!("w{}", self.next_id), method);
        self.stream.write_all(request.to_line().as_bytes())?;
        self.stream.flush()?;

        let mut response = String::new();
        if self.reader.read_line(&mut response)? == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "daemon closed the connection",
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use amon_protocol::AgentState;

    fn entry() -> AgentEntry {
        AgentEntry {
            id: "a1".into(),
            agent: "claude".into(),
            state: AgentState::Working,
            state_since: 1,
            cwd: "/work".into(),
            pid: 42,
            args: vec!["claude".into()],
            hostname: "far".into(),
            started_at: 1,
            title: None,
            agent_session_id: None,
            agent_session_path: None,
            window: None,
            workspace: None,
            branch: None,
            focused: None,
            seen: None,
        }
    }

    #[test]
    fn nothing_is_whispered_before_the_answer() {
        let outbox = whisper::Outbox::default();
        let tee = Tee {
            outbox: Some(outbox.clone()),
        };
        let held = entry();
        tee.heartbeat(Some(&held));
        tee.event(Method::AgentUpdate(AgentPatch::new("a1")));
        assert!(outbox.drain().is_empty());
    }

    #[test]
    fn arming_whispers_the_held_entry() {
        let outbox = whisper::Outbox::default();
        let tee = Tee {
            outbox: Some(outbox.clone()),
        };
        tee.armed(Some(&entry()));
        let queued = outbox.drain();
        assert_eq!(queued.len(), 1, "the snapshot register, nothing else");
        let expected = whisper::encode(&whisper::WhisperFrame::Event(Box::new(
            Method::AgentRegister(entry()),
        )));
        assert_eq!(queued[0], expected);
    }

    #[test]
    fn an_armed_tee_whispers_updates_and_heartbeats() {
        let outbox = whisper::Outbox::default();
        let tee = Tee {
            outbox: Some(outbox.clone()),
        };
        tee.armed(None);
        tee.event(Method::AgentUpdate(AgentPatch::new("a1")));
        tee.heartbeat(Some(&entry()));
        assert_eq!(outbox.drain().len(), 2);
    }

    #[test]
    fn a_wrapper_without_an_outbox_stays_silent() {
        let tee = Tee { outbox: None };
        tee.armed(Some(&entry()));
        tee.heartbeat(Some(&entry()));
        // Nothing to assert against — the absence of a panic and of any
        // queue is the behavior.
    }

    #[test]
    fn the_outbox_hands_back_what_was_queued_in_order() {
        let outbox = whisper::Outbox::default();
        outbox.enqueue(b"one".to_vec());
        outbox.enqueue(b"two".to_vec());
        assert_eq!(outbox.drain(), vec![b"one".to_vec(), b"two".to_vec()]);
        assert!(outbox.drain().is_empty(), "drained means gone");
    }

    #[test]
    fn arming_is_visible_across_clones() {
        let outbox = whisper::Outbox::default();
        let clone = outbox.clone();
        assert!(!clone.armed());
        outbox.arm();
        assert!(clone.armed());
    }
}
