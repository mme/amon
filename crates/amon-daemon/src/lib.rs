//! The daemon: one per user, holding the live registry of wrapped agents.
//!
//! It is started on demand by any amon command and then stays until something
//! kills it — including a newer amon, which takes the socket over so an
//! upgrade applies without anyone hunting for processes. Wrappers reconnect
//! and re-register, so a takeover costs a moment of churn and nothing else
//! (ADR-0002).

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use amon_protocol::{
    paths, ConfigResult, Error, ErrorCode, Event, Hello, HelloResult, Method, ParseError, Request,
    Response, Role, StatusResult, PROTOCOL_VERSION,
};

mod config;
mod manifests;
mod registry;
mod runtime;
mod sound;
mod updates;

pub use registry::Registry;

/// Where the user's configuration lives.
///
/// Exposed from here because the daemon is the only thing that reads it
/// (ADR-0012), so it is the only thing that should be believed about where it
/// is. The CLI needs it to write a starter file, and a second definition of
/// that path is a second thing to get wrong.
pub fn config_path() -> Option<std::path::PathBuf> {
    config::path()
}

/// Runs the daemon until it is asked to shut down, taking the socket over from
/// an older daemon if one holds it.
pub fn run(version: String) -> std::io::Result<()> {
    // Logs go to stderr: visible when the daemon runs in the foreground,
    // discarded when a wrapper auto-started it against /dev/null.
    let _ = tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .try_init();

    let socket = paths::daemon_socket();
    if let Some(parent) = socket.parent() {
        paths::ensure_private_dir(parent)?;
    }

    let listener = bind(&socket, &version)?;
    // Remembered now so cleanup can tell "still our file" from "a successor's":
    // the listener fd itself is no help, since fstat on a socket reports the
    // anonymous sockfs inode, never the bound path's.
    let bound_socket = socket_file_identity(&socket);
    manifests::refresh_periodically();

    let registry = Registry::new();
    // Read once here and polled from then on. Every change is pushed to
    // subscribers, which is what lets the bar reconfigure without a restart.
    let config = config::Watcher::start({
        let registry = registry.clone();
        move |config| registry.broadcast_config(config.clone())
    });
    updates::check_periodically(registry.clone(), config.clone(), version.clone());
    let shutdown = Arc::new(AtomicBool::new(false));
    // Shared with the herdr watcher: projected entries and socket
    // connections draw registry ids from one counter, because the registry
    // keys ownership by connection id and two allocators would collide.
    let next_connection = Arc::new(AtomicU64::new(0));

    // Agents inside runtime sessions (herdr, luvus) surface through the same
    // registry as wrapped ones. AMON_RUNTIMES=0 exists for tests: an e2e
    // daemon on a machine with a live session must not see the developer's
    // real agents.
    if std::env::var_os("AMON_RUNTIMES").is_none_or(|value| value != "0") {
        runtime::spawn(registry.clone(), next_connection.clone());
    }

    for stream in listener.incoming() {
        if shutdown.load(Ordering::Relaxed) {
            break;
        }
        let Ok(stream) = stream else { continue };

        let registry = registry.clone();
        let config = config.clone();
        let version = version.clone();
        let shutdown = shutdown.clone();
        let connection = next_connection.fetch_add(1, Ordering::Relaxed);
        std::thread::spawn(move || {
            serve(connection, stream, &registry, &config, &version, &shutdown);
            // However the connection ended, everything it owned goes with it.
            registry.disconnect(connection);
        });
    }

    // Remove the socket file only if it is still this daemon's. During a
    // takeover the successor unlinks the path and binds its own socket there;
    // unlinking unconditionally would then delete the *new* daemon's socket,
    // leaving it listening on an unreachable inode.
    if bound_socket.is_some() && socket_file_identity(&socket) == bound_socket {
        let _ = std::fs::remove_file(&socket);
    }
    Ok(())
}

/// The (device, inode) of the socket *file*, which is what distinguishes the
/// file this daemon bound from one a successor bound at the same path.
fn socket_file_identity(socket: &Path) -> Option<(u64, u64)> {
    use std::os::unix::fs::MetadataExt;

    let meta = std::fs::symlink_metadata(socket).ok()?;
    Some((meta.dev(), meta.ino()))
}

/// Binds the daemon socket, replacing an older daemon if one answers.
///
/// The socket path is the only rendezvous, so binding it is what decides who
/// is the daemon; two wrappers racing to start one cannot both win.
fn bind(socket: &Path, version: &str) -> std::io::Result<UnixListener> {
    match UnixListener::bind(socket) {
        Ok(listener) => return Ok(listener),
        Err(error) if error.kind() != std::io::ErrorKind::AddrInUse => return Err(error),
        Err(_) => {}
    }

    match ask_existing_daemon_to_exit(socket, version) {
        DaemonAtSocket::Newer => Err(std::io::Error::other(
            "a amon daemon of the same or newer version is already running",
        )),
        // Either it agreed to exit, or nothing was listening and the socket
        // file is a leftover from a daemon that was killed.
        DaemonAtSocket::Replaced | DaemonAtSocket::Stale => {
            for _ in 0..40 {
                if let Ok(listener) = UnixListener::bind(socket) {
                    return Ok(listener);
                }
                let _ = std::fs::remove_file(socket);
                std::thread::sleep(Duration::from_millis(25));
            }
            UnixListener::bind(socket)
        }
    }
}

enum DaemonAtSocket {
    /// Answered and agreed to exit.
    Replaced,
    /// Answered and is at least as new as us; we must not displace it.
    Newer,
    /// Nothing answered; the socket file is left over.
    Stale,
}

fn ask_existing_daemon_to_exit(socket: &Path, version: &str) -> DaemonAtSocket {
    let Ok(stream) = UnixStream::connect(socket) else {
        return DaemonAtSocket::Stale;
    };
    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(2)));
    let Ok(write_half) = stream.try_clone() else {
        return DaemonAtSocket::Stale;
    };
    let mut reader = BufReader::new(stream);
    let mut writer = &write_half;

    let hello = Request::new(
        "takeover-hello",
        Method::Hello(Hello {
            role: Role::Client,
            protocol: PROTOCOL_VERSION,
            version: version.to_string(),
        }),
    );
    if writer.write_all(hello.to_line().as_bytes()).is_err() {
        return DaemonAtSocket::Stale;
    }

    let mut line = String::new();
    if reader.read_line(&mut line).unwrap_or(0) == 0 {
        return DaemonAtSocket::Stale;
    }
    let running = serde_json::from_str::<Response>(&line)
        .ok()
        .and_then(|response| response.result)
        .and_then(|result| serde_json::from_value::<HelloResult>(result).ok());

    // Only a strictly newer build displaces a running daemon: an equal version
    // means the daemon already is what we would start.
    if let Some(running) = running {
        if !is_newer(version, &running.version) {
            return DaemonAtSocket::Newer;
        }
    }

    let shutdown = Request::new("takeover-shutdown", Method::DaemonShutdown);
    if writer.write_all(shutdown.to_line().as_bytes()).is_err() {
        return DaemonAtSocket::Stale;
    }
    let mut line = String::new();
    let _ = reader.read_line(&mut line);
    DaemonAtSocket::Replaced
}

/// Compares dotted version strings numerically, so 0.10.0 beats 0.9.0.
/// Unparseable components count as 0, which keeps a malformed version from
/// squatting on the socket.
pub(crate) fn is_newer(candidate: &str, running: &str) -> bool {
    fn parts(version: &str) -> Vec<u64> {
        version
            .split(['.', '-', '+'])
            .map(|part| part.parse().unwrap_or(0))
            .collect()
    }
    parts(candidate) > parts(running)
}

/// One connection's write half, shared between the request/response loop and
/// a subscriber's event pusher. The lock is what keeps an event from landing
/// in the middle of a response line — both are newline-delimited JSON on the
/// same stream.
type SharedWriter = Arc<Mutex<UnixStream>>;

fn write_line(writer: &SharedWriter, line: &str) -> std::io::Result<()> {
    let mut stream = writer
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    stream.write_all(line.as_bytes())?;
    stream.flush()
}

fn serve(
    connection: registry::ConnectionId,
    stream: UnixStream,
    registry: &Registry,
    config: &config::Watcher,
    version: &str,
    shutdown: &AtomicBool,
) {
    let Ok(write_half) = stream.try_clone() else {
        return;
    };
    let writer: SharedWriter = Arc::new(Mutex::new(write_half));
    let reader = BufReader::new(stream);

    for line in reader.lines().map_while(Result::ok) {
        if line.trim().is_empty() {
            continue;
        }

        let response = match Request::parse(&line) {
            Ok(request) => {
                let id = request.id.clone();
                match handle(connection, request, registry, config, version, &writer) {
                    Outcome::Reply(reply) => reply(id),
                    Outcome::Shutdown => {
                        shutdown.store(true, Ordering::Relaxed);
                        let response = Response::ack(id);
                        let _ = write_line(&writer, &response.to_line());
                        // Wake the accept loop so it notices the flag.
                        let _ = UnixStream::connect(paths::daemon_socket());
                        return;
                    }
                }
            }
            // Answer rather than hang up: a wrapper built against a newer amon
            // should learn its request was not understood and carry on.
            Err(ParseError {
                id: Some(id),
                error,
            }) => Response::err(id, error),
            Err(ParseError { id: None, error }) => Response::err("", error),
        };

        if write_line(&writer, &response.to_line()).is_err() {
            return;
        }
    }
}

type Reply = Box<dyn FnOnce(String) -> Response + Send>;

enum Outcome {
    Reply(Reply),
    Shutdown,
}

fn handle(
    connection: registry::ConnectionId,
    request: Request,
    registry: &Registry,
    config: &config::Watcher,
    version: &str,
    writer: &SharedWriter,
) -> Outcome {
    match request.method {
        Method::Hello(_) => {
            let result = HelloResult {
                protocol: PROTOCOL_VERSION,
                version: version.to_string(),
            };
            Outcome::Reply(Box::new(move |id| Response::ok(id, &result)))
        }
        Method::AgentRegister(entry) => {
            registry.register(connection, entry);
            Outcome::Reply(Box::new(Response::ack))
        }
        Method::AgentUpdate(patch) => {
            if registry.update(connection, &patch) {
                Outcome::Reply(Box::new(Response::ack))
            } else {
                Outcome::Reply(Box::new(move |id| {
                    Response::err(
                        id,
                        Error::new(
                            ErrorCode::UnknownAgent,
                            "this connection has not registered an agent",
                        ),
                    )
                }))
            }
        }
        Method::Status => {
            let result = StatusResult {
                agents: registry.agents(),
            };
            Outcome::Reply(Box::new(move |id| Response::ok(id, &result)))
        }
        Method::Config => {
            let loaded = config.loaded();
            let result = ConfigResult {
                config: loaded.config,
                error: loaded.error,
            };
            Outcome::Reply(Box::new(move |id| Response::ok(id, &result)))
        }
        Method::Subscribe => {
            let (events, inbox) = std::sync::mpsc::channel();
            registry.subscribe(connection, events);
            let writer = writer.clone();
            std::thread::spawn(move || push_events(writer, inbox));
            Outcome::Reply(Box::new(Response::ack))
        }
        Method::DaemonShutdown => Outcome::Shutdown,
        // Hook reports belong to a wrapper's socket, not here.
        Method::AgentReportState(_)
        | Method::AgentReportSession(_)
        | Method::AgentReportActivity(_) => Outcome::Reply(Box::new(move |id| {
            Response::err(
                id,
                Error::new(
                    ErrorCode::UnsupportedMethod,
                    "hook reports go to the wrapper's socket, not the daemon",
                ),
            )
        })),
    }
}

fn push_events(writer: SharedWriter, inbox: std::sync::mpsc::Receiver<Event>) {
    for event in inbox {
        let Ok(mut line) = serde_json::to_string(&event) else {
            continue;
        };
        line.push('\n');
        if write_line(&writer, &line).is_err() {
            return;
        }
    }
}
