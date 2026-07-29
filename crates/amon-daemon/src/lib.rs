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
use std::sync::Arc;
use std::time::Duration;

use amon_protocol::{
    paths, Error, ErrorCode, Event, Hello, HelloResult, Method, ParseError, Request, Response,
    Role, StatusResult, PROTOCOL_VERSION,
};

mod manifests;
mod registry;

pub use registry::Registry;

/// Runs the daemon until it is asked to shut down, taking the socket over from
/// an older daemon if one holds it.
pub fn run(version: String) -> std::io::Result<()> {
    let socket = paths::daemon_socket();
    if let Some(parent) = socket.parent() {
        paths::ensure_private_dir(parent)?;
    }

    let listener = bind(&socket, &version)?;
    manifests::refresh_periodically();

    let registry = Registry::new();
    let shutdown = Arc::new(AtomicBool::new(false));
    let next_connection = AtomicU64::new(0);

    for stream in listener.incoming() {
        if shutdown.load(Ordering::Relaxed) {
            break;
        }
        let Ok(stream) = stream else { continue };

        let registry = registry.clone();
        let version = version.clone();
        let shutdown = shutdown.clone();
        let connection = next_connection.fetch_add(1, Ordering::Relaxed);
        std::thread::spawn(move || {
            serve(connection, stream, &registry, &version, &shutdown);
            // However the connection ended, everything it owned goes with it.
            registry.disconnect(connection);
        });
    }

    let _ = std::fs::remove_file(&socket);
    Ok(())
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
            "a newer amon daemon is already running",
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
fn is_newer(candidate: &str, running: &str) -> bool {
    fn parts(version: &str) -> Vec<u64> {
        version
            .split(['.', '-', '+'])
            .map(|part| part.parse().unwrap_or(0))
            .collect()
    }
    parts(candidate) > parts(running)
}

fn serve(
    connection: registry::ConnectionId,
    stream: UnixStream,
    registry: &Registry,
    version: &str,
    shutdown: &AtomicBool,
) {
    let Ok(write_half) = stream.try_clone() else {
        return;
    };
    let reader = BufReader::new(stream);
    let mut writer = &write_half;

    for line in reader.lines().map_while(Result::ok) {
        if line.trim().is_empty() {
            continue;
        }

        let response = match Request::parse(&line) {
            Ok(request) => {
                let id = request.id.clone();
                match handle(connection, request, registry, version, &write_half) {
                    Outcome::Reply(reply) => reply(id),
                    Outcome::Shutdown => {
                        shutdown.store(true, Ordering::Relaxed);
                        let response = Response::ack(id);
                        let _ = writer.write_all(response.to_line().as_bytes());
                        let _ = writer.flush();
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

        if writer.write_all(response.to_line().as_bytes()).is_err() {
            return;
        }
        let _ = writer.flush();
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
    version: &str,
    stream: &UnixStream,
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
        Method::Subscribe => {
            let (events, inbox) = std::sync::mpsc::channel();
            registry.subscribe(connection, events);
            if let Ok(stream) = stream.try_clone() {
                std::thread::spawn(move || push_events(stream, inbox));
            }
            Outcome::Reply(Box::new(Response::ack))
        }
        Method::DaemonShutdown => Outcome::Shutdown,
        // Hook reports belong to a wrapper's socket, not here.
        Method::AgentReportState(_) | Method::AgentReportSession(_) => {
            Outcome::Reply(Box::new(move |id| {
                Response::err(
                    id,
                    Error::new(
                        ErrorCode::UnsupportedMethod,
                        "hook reports go to the wrapper's socket, not the daemon",
                    ),
                )
            }))
        }
    }
}

fn push_events(stream: UnixStream, inbox: std::sync::mpsc::Receiver<Event>) {
    let mut stream = &stream;
    for event in inbox {
        let Ok(mut line) = serde_json::to_string(&event) else {
            continue;
        };
        line.push('\n');
        if stream.write_all(line.as_bytes()).is_err() {
            return;
        }
        let _ = stream.flush();
    }
}
