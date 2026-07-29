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

use amon_protocol::{
    paths, AgentEntry, AgentPatch, Hello, Method, Request, Role, PROTOCOL_VERSION,
};

const FIRST_RETRY: Duration = Duration::from_millis(100);
const MAX_RETRY: Duration = Duration::from_secs(30);
/// Long enough that a wedged daemon cannot stall the link thread behind the
/// agent's own progress, short enough to notice a dead socket promptly.
const IO_TIMEOUT: Duration = Duration::from_secs(2);

pub enum Message {
    Register(Box<AgentEntry>),
    Update(Box<AgentPatch>),
}

/// Handle held by the rest of the wrapper. Sending never blocks and never
/// fails in a way the caller must handle: if the link thread is gone, the
/// message is dropped.
#[derive(Clone)]
pub struct DaemonLink {
    tx: mpsc::Sender<Message>,
}

impl DaemonLink {
    pub fn spawn(version: String) -> Self {
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || run(rx, version));
        Self { tx }
    }

    pub fn register(&self, entry: AgentEntry) {
        let _ = self.tx.send(Message::Register(Box::new(entry)));
    }

    pub fn update(&self, patch: AgentPatch) {
        let _ = self.tx.send(Message::Update(Box::new(patch)));
    }
}

fn run(rx: mpsc::Receiver<Message>, version: String) {
    let mut entry: Option<AgentEntry> = None;
    let mut connection: Option<Connection> = None;
    let mut retry = FIRST_RETRY;

    loop {
        // When connected there is nothing to retry, so block until there is
        // something to send; otherwise wake up to try reconnecting.
        let wait = if connection.is_some() {
            Duration::from_secs(3600)
        } else {
            retry
        };

        match rx.recv_timeout(wait) {
            Ok(Message::Register(new_entry)) => {
                entry = Some(*new_entry);
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
                    if let Some(link) = connection.as_mut() {
                        if link.send(Method::AgentUpdate(*patch)).is_err() {
                            connection = None;
                        }
                    }
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
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

/// Connects, starting a daemon if none is listening. Races are settled by the
/// socket itself: whoever binds first is the daemon and everyone else's
/// connect succeeds against it.
fn connect(version: &str) -> Option<Connection> {
    if let Some(link) = Connection::open(version) {
        return Some(link);
    }
    spawn_daemon();
    // Give the new daemon a moment to bind before deciding it failed; the
    // caller's backoff covers the case where it never does.
    for _ in 0..20 {
        std::thread::sleep(Duration::from_millis(25));
        if let Some(link) = Connection::open(version) {
            return Some(link);
        }
    }
    None
}

fn spawn_daemon() {
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    let _ = std::process::Command::new(exe)
        .arg("daemon")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
}

struct Connection {
    stream: UnixStream,
    reader: BufReader<UnixStream>,
    next_id: u64,
}

impl Connection {
    fn open(version: &str) -> Option<Self> {
        let stream = UnixStream::connect(paths::daemon_socket()).ok()?;
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
