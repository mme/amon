//! Finding a runtime's attached clients on this machine by reading /proc.
//!
//! Both runtimes leave argv verbatim and never rewrite their process title,
//! which is what makes classification by argv safe — the runtime's own
//! [`Hosted::classify`] does the judging. A session is shown exactly while
//! it has an attached client (a spec decision — a headless server's agents
//! have no window to jump to), and of several clients the lowest pid wins:
//! "first, or something" was the whole requirement.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use super::{Hosted, Role, Session};

/// Only sessions whose socket lies under this directory are adopted, when
/// set. An e2e daemon on a developer's desktop must not adopt the
/// developer's live sessions; the sandbox names the one root it wants seen.
pub const ROOT_ENV: &str = "AMON_RUNTIMES_ROOT";

/// One variable out of a NUL-separated environ blob.
pub fn env_value<'a>(environ: &'a str, key: &str) -> Option<&'a str> {
    environ
        .split('\0')
        .filter_map(|pair| pair.split_once('='))
        .find(|(name, _)| *name == key)
        .map(|(_, value)| value)
        .filter(|value| !value.is_empty())
}

/// `(session id, tty_nr)` from a `/proc/<pid>/stat` line — fields 6 and 7,
/// counted after the last `)` because comm is arbitrary bytes.
pub fn stat_fields(stat: &str) -> Option<(u32, u32)> {
    let after_comm = &stat[stat.rfind(')')? + 1..];
    let mut fields = after_comm.split_whitespace();
    let session = fields.nth(3)?.parse().ok()?;
    let tty_nr = fields.next()?.parse().ok()?;
    Some((session, tty_nr))
}

/// Records a client against its socket, keeping the lowest pid.
///
/// Sessions are told apart by socket, not by name: two nameless clients
/// pointed at different sockets are two sessions, and two clients on one
/// socket are one session with a shared view — of which amon follows the
/// first, which is all the tie-breaking a shared view deserves.
fn claim(found: &mut Vec<(PathBuf, u32)>, socket: PathBuf, pid: u32) {
    match found.iter_mut().find(|(known, _)| *known == socket) {
        Some((_, client)) => *client = (*client).min(pid),
        None => found.push((socket, pid)),
    }
}

fn under_root(socket: &Path, root: Option<&Path>) -> bool {
    root.is_none_or(|root| socket.starts_with(root))
}

/// Every session of runtime `H` that has an attached client right now.
pub fn sessions<H: Hosted>() -> Vec<Session> {
    use std::os::unix::fs::MetadataExt;

    let root = std::env::var_os(ROOT_ENV).map(PathBuf::from);
    let mut found: Vec<(PathBuf, u32)> = Vec::new();
    let mut names: HashMap<PathBuf, Option<String>> = HashMap::new();
    let user = unsafe { libc::geteuid() };
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return Vec::new();
    };
    for entry in entries.flatten() {
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|name| name.parse::<u32>().ok())
        else {
            continue;
        };
        let path = entry.path();
        // Somebody else's runtime is none of this daemon's business — amon is
        // per-user. It matters beyond tidiness: `comm`, `cmdline` and `stat`
        // are world-readable while `environ` is not, so another user's client
        // would be classified from an empty environment, fall back to *this*
        // user's default socket, and claim the session with a pid whose
        // window lives in another login entirely.
        let Ok(owner) = std::fs::metadata(&path) else {
            continue;
        };
        if owner.uid() != user {
            continue;
        }
        // comm first: cheap, and filters out everything that is not the
        // runtime before argv and environ are read at all.
        let Ok(comm) = std::fs::read_to_string(path.join("comm")) else {
            continue;
        };
        if comm.trim_end() != H::COMM {
            continue;
        }
        let Ok(cmdline) = std::fs::read(path.join("cmdline")) else {
            continue;
        };
        let argv: Vec<String> = cmdline
            .split(|byte| *byte == 0)
            .filter(|part| !part.is_empty())
            .map(|part| String::from_utf8_lossy(part).into_owned())
            .collect();
        let Ok(stat) = std::fs::read_to_string(path.join("stat")) else {
            continue;
        };
        let Some((session_id, tty_nr)) = stat_fields(&stat) else {
            continue;
        };
        let environ = std::fs::read(path.join("environ")).unwrap_or_default();
        let environ = String::from_utf8_lossy(&environ).into_owned();
        if let Role::Client { name } = H::classify(&argv, session_id == pid, tty_nr != 0, &environ)
        {
            let Some(socket) = H::socket_for(&argv, &environ) else {
                continue;
            };
            if !under_root(&socket, root.as_deref()) {
                continue;
            }
            names.entry(socket.clone()).or_insert(name);
            claim(&mut found, socket, pid);
        }
    }
    found
        .into_iter()
        .map(|(socket, client_pid)| Session {
            name: names.get(&socket).cloned().unwrap_or_default(),
            socket,
            client_pid,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_stat_line_yields_sid_and_tty_past_a_hostile_comm() {
        // comm may contain spaces and parens; fields count from after the
        // last ')'. Layout: pid (comm) state ppid pgrp session tty_nr …
        let stat = "17 (h (x) dr) S 1 17 17 34816 1234";
        assert_eq!(stat_fields(stat), Some((17, 34816)));
        assert_eq!(stat_fields("garbage"), None);
    }

    #[test]
    fn two_clients_on_one_socket_are_one_session_and_the_first_wins() {
        // Same socket, two attached clients: a shared view, so amon follows
        // one of them — the lowest pid, deterministically. Sessions are kept
        // apart by socket rather than by name, because two nameless clients
        // pointed at different sockets are two sessions, not one.
        let mut found: Vec<(PathBuf, u32)> = Vec::new();
        for (socket, pid) in [("/a.sock", 40u32), ("/a.sock", 12), ("/b.sock", 30)] {
            claim(&mut found, PathBuf::from(socket), pid);
        }
        found.sort();
        assert_eq!(
            found,
            vec![
                (PathBuf::from("/a.sock"), 12),
                (PathBuf::from("/b.sock"), 30)
            ]
        );
    }

    #[test]
    fn a_root_limits_which_sockets_are_adopted() {
        let root = Some(Path::new("/tmp/sb"));
        assert!(under_root(
            Path::new("/tmp/sb/home/.config/herdr/herdr.sock"),
            root
        ));
        assert!(!under_root(
            Path::new("/home/me/.config/herdr/herdr.sock"),
            root
        ));
        assert!(
            under_root(Path::new("/home/me/.config/herdr/herdr.sock"), None),
            "no root: everything"
        );
    }
}
