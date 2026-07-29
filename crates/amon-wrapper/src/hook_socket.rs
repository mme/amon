//! Where the agent's installed hooks report.
//!
//! Each wrapper listens on its own socket and tells its agent the path through
//! `AMON_SOCKET_PATH`, so a hook always reaches the wrapper that spawned it —
//! which is the process holding the state machine that has to reconcile hook
//! reports with screen detection (ADR-0001).
//!
//! Hooks connect, send one line, and hang up. These connections never own a
//! registry entry and dropping one means nothing.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;

use amon_protocol::{paths, Method, ParseError, Request, Response};

use crate::observer::Signal;

/// Removes the socket file when the wrapper exits, so a stale path never
/// misleads the next hook into thinking someone is listening.
pub struct HookSocket {
    path: PathBuf,
}

impl HookSocket {
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for HookSocket {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Binds the agent's socket and serves it on a background thread. Returns
/// `None` if the socket cannot be created: hooks are an optional signal, and
/// losing them is not a reason to refuse to run the agent.
pub fn listen(agent_id: &str, signals: Sender<Signal>) -> Option<HookSocket> {
    let path = paths::agent_socket(agent_id);
    paths::ensure_private_dir(path.parent()?).ok()?;
    // A leftover file from a crashed wrapper would make bind fail.
    let _ = std::fs::remove_file(&path);

    let listener = UnixListener::bind(&path).ok()?;
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let signals = signals.clone();
            // One short-lived connection per report; a hook that stalls must
            // not hold up the next one.
            std::thread::spawn(move || serve(stream, &signals));
        }
    });

    Some(HookSocket { path })
}

fn serve(stream: UnixStream, signals: &Sender<Signal>) {
    let Ok(write_half) = stream.try_clone() else {
        return;
    };
    let reader = BufReader::new(stream);

    for line in reader.lines().map_while(Result::ok) {
        if line.trim().is_empty() {
            continue;
        }
        let response = match Request::parse(&line) {
            Ok(request) => {
                let id = request.id.clone();
                handle(request, signals);
                Response::ack(id)
            }
            // An unknown method from a newer hook still gets an answer, so the
            // hook's read succeeds and it exits quietly rather than hanging.
            Err(ParseError {
                id: Some(id),
                error,
            }) => Response::err(id, error),
            Err(ParseError { id: None, error }) => Response::err("", error),
        };
        let mut write_half = &write_half;
        if write_half.write_all(response.to_line().as_bytes()).is_err() {
            return;
        }
        let _ = write_half.flush();
    }
}

fn handle(request: Request, signals: &Sender<Signal>) {
    match request.method {
        Method::AgentReportState(report) => {
            let _ = signals.send(Signal::HookState {
                source: report.source,
                agent: report.agent,
                state: report.state,
                seq: Some(report.seq),
                session_id: report.agent_session_id,
            });
        }
        Method::AgentReportSession(report) => {
            let _ = signals.send(Signal::HookSession {
                source: report.source,
                agent: report.agent,
                session_id: report.agent_session_id,
                session_path: report.agent_session_path,
                seq: Some(report.seq),
                session_start_source: report.session_start_source,
            });
        }
        // Everything else belongs to the daemon's socket, not this one.
        _ => {}
    }
}
