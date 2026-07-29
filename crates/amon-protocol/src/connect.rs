//! Reaching the daemon, starting one if none is listening.
//!
//! The wrapper's daemon link and the one-shot CLI commands need the same
//! startup primitive (ADR-0002): try the socket, spawn `amon daemon` if
//! nothing answers, and give the newcomer a moment to bind. It lives here,
//! next to the socket paths, so the two callers cannot drift apart.

use std::os::unix::net::UnixStream;
use std::time::Duration;

use crate::paths;

/// Connects to the daemon's socket, starting a daemon first if none is
/// listening. Returns `None` when no daemon could be reached or started;
/// whether and when to retry is the caller's policy.
///
/// Races are settled by the socket itself: whoever binds first is the daemon,
/// and everyone else's connect succeeds against it.
pub fn connect_or_spawn_daemon() -> Option<UnixStream> {
    if let Ok(stream) = UnixStream::connect(paths::daemon_socket()) {
        return Some(stream);
    }
    spawn_daemon();
    for _ in 0..40 {
        std::thread::sleep(Duration::from_millis(25));
        if let Ok(stream) = UnixStream::connect(paths::daemon_socket()) {
            return Some(stream);
        }
    }
    None
}

/// Spawns `amon daemon` detached from this process's stdio. The daemon is a
/// subcommand of the one amon binary, so the current executable is it.
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
