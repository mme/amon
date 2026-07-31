//! Which window the agent is running in.
//!
//! Hyprland names windows by address and tells anyone who asks over two unix
//! sockets: one for queries, one that streams events. The wrapper finds its own
//! window once, by walking its process ancestry until it reaches a process the
//! compositor knows as a window, and then follows that window's workspace.
//!
//! Everything here is best-effort and silent. No compositor, an unreadable
//! socket, or an ancestry that matches nothing simply means the wrapper reports
//! no window — never a guess, because a wrong window is worse than none, and
//! never an error, because amon is not worth interrupting a session over.

use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;
use std::time::Duration;

use crate::observer::Signal;

/// Bounds the ancestry walk. Deeper than any real terminal → shell → agent
/// chain, and a hard stop if `/proc` ever hands back a cycle.
const MAX_ANCESTRY: usize = 32;
const IO_TIMEOUT: Duration = Duration::from_secs(2);
/// Caps on what the compositor can make the wrapper hold. Both are far above
/// anything Hyprland sends — a desktop full of windows is tens of kilobytes of
/// JSON, an event line a few hundred bytes — and exist so that a peer which
/// never stops talking cannot grow the wrapper instead of failing.
const MAX_RESPONSE: u64 = 8 * 1024 * 1024;
const MAX_EVENT_LINE: u64 = 64 * 1024;

/// The window an agent is running in, as the compositor sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Window {
    /// Hyprland's window address, normalized (no `0x`, lowercase). Opaque to
    /// everyone but the compositor.
    pub address: String,
    pub workspace: Option<String>,
}

/// Starts following this wrapper's window, if there is one to follow.
///
/// Returns without doing anything when the compositor is absent or the window
/// cannot be identified; the thread it spawns lives as long as the process.
pub fn spawn(signals: Sender<Signal>) {
    std::thread::spawn(move || {
        let Some(directory) = socket_dir() else {
            return;
        };
        // Subscribed to before the window is looked up, so that a move in
        // between is waiting in the socket rather than lost: events queue from
        // the moment of connection. The cost is that those queued events are
        // replayed after the snapshot, so two moves during the lookup are
        // reported newest, older, newest — briefly stale rather than wrong
        // forever, and a subscriber may see the middle one.
        let Ok(events) = UnixStream::connect(directory.join(".socket2.sock")) else {
            return;
        };
        let Some(window) = resolve(&directory, std::process::id()) else {
            return;
        };

        let _ = signals.send(Signal::Window {
            window: Some(window.address.clone()),
            workspace: window.workspace.clone(),
        });

        follow(&directory, events, window, &signals);
    });
}

/// Reads the event stream, keeping the window's workspace current.
fn follow(directory: &Path, events: UnixStream, mut window: Window, signals: &Sender<Signal>) {
    let mut reader = BufReader::new(events);
    let mut line = String::new();
    loop {
        line.clear();
        // Bounded, so an event line that never ends cannot grow without limit.
        // An over-long line is cut, fails to parse, and the next read resumes
        // mid-line — which is a dropped event, not a dead wrapper.
        match reader.by_ref().take(MAX_EVENT_LINE).read_line(&mut line) {
            Ok(0) | Err(_) => return,
            Ok(_) => {}
        }
        match parse_event(line.trim_end()) {
            Some(Event::Moved { address, workspace }) if address == window.address => {
                if window.workspace.as_deref() == Some(workspace.as_str()) {
                    continue;
                }
                window.workspace = Some(workspace.clone());
                let _ = signals.send(Signal::Window {
                    window: Some(window.address.clone()),
                    workspace: Some(workspace),
                });
            }
            // The window we were following is gone. A terminal can outlive one
            // of its windows, so look once for a new one rather than assuming
            // the agent is on its way out. Failing that, the workspace goes
            // with the window: a workspace without a window to be on is a
            // stale answer, not a partial one.
            Some(Event::Closed { address }) if address == window.address => {
                match resolve(directory, std::process::id()) {
                    Some(found) => {
                        window = found;
                        let _ = signals.send(Signal::Window {
                            window: Some(window.address.clone()),
                            workspace: window.workspace.clone(),
                        });
                    }
                    None => {
                        let _ = signals.send(Signal::Window {
                            window: None,
                            workspace: None,
                        });
                        return;
                    }
                }
            }
            _ => {}
        }
    }
}

/// Finds the window hosting `pid` by walking up its ancestry.
fn resolve(directory: &Path, pid: u32) -> Option<Window> {
    let clients = request(directory, "j/clients")?;
    window_for_ancestry(&clients, &ancestry(pid))
}

/// The nearest ancestor the compositor knows as a window.
///
/// Nearest wins so that a terminal running inside a terminal resolves to the
/// inner one. A process owning several windows is ambiguous — that is a `None`,
/// not a coin flip.
pub fn window_for_ancestry(clients_json: &str, ancestry: &[u32]) -> Option<Window> {
    let clients: serde_json::Value = serde_json::from_str(clients_json).ok()?;
    let clients = clients.as_array()?;

    for pid in ancestry {
        let mut windows = clients.iter().filter(|client| {
            client.get("pid").and_then(serde_json::Value::as_u64) == Some(u64::from(*pid))
        });
        let Some(client) = windows.next() else {
            continue;
        };
        if windows.next().is_some() {
            return None;
        }
        let address = client.get("address").and_then(serde_json::Value::as_str)?;
        return Some(Window {
            address: normalize(address),
            workspace: client
                .get("workspace")
                .and_then(|workspace| workspace.get("name"))
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned),
        });
    }
    None
}

/// `pid` and its ancestors, nearest first, stopping at init.
fn ancestry(pid: u32) -> Vec<u32> {
    let mut chain = vec![pid];
    let mut current = pid;
    for _ in 0..MAX_ANCESTRY {
        let Ok(stat) = std::fs::read_to_string(format!("/proc/{current}/stat")) else {
            break;
        };
        let Some(parent) = parse_ppid(&stat) else {
            break;
        };
        if parent <= 1 {
            break;
        }
        chain.push(parent);
        current = parent;
    }
    chain
}

/// The parent pid out of a `/proc/<pid>/stat` line.
///
/// Everything up to the last `)` is skipped: the command name sits in
/// parentheses and may itself contain spaces and parentheses, which is what
/// makes splitting the line from the left wrong.
pub fn parse_ppid(stat: &str) -> Option<u32> {
    let after_comm = &stat[stat.rfind(')')? + 1..];
    after_comm.split_whitespace().nth(1)?.parse().ok()
}

/// One line of Hyprland's event stream, if it is one we care about.
#[derive(Debug, PartialEq, Eq)]
pub enum Event {
    Moved { address: String, workspace: String },
    Closed { address: String },
}

/// Parses an event line.
///
/// Addresses arrive here without the `0x` the query socket puts on them, so
/// both are normalized before anything is compared.
pub fn parse_event(line: &str) -> Option<Event> {
    let (name, payload) = line.split_once(">>")?;
    match name {
        "movewindow" => {
            let (address, workspace) = payload.split_once(',')?;
            Some(Event::Moved {
                address: normalize(address),
                workspace: workspace.to_owned(),
            })
        }
        // Same event, with the workspace id wedged in the middle. The name is
        // last and may itself contain commas, so it is taken as the remainder.
        "movewindowv2" => {
            let (address, rest) = payload.split_once(',')?;
            let (_id, workspace) = rest.split_once(',')?;
            Some(Event::Moved {
                address: normalize(address),
                workspace: workspace.to_owned(),
            })
        }
        "closewindow" => Some(Event::Closed {
            address: normalize(payload),
        }),
        _ => None,
    }
}

fn normalize(address: &str) -> String {
    address.trim().trim_start_matches("0x").to_ascii_lowercase()
}

/// Where this Hyprland instance keeps its sockets, if it is running.
fn socket_dir() -> Option<PathBuf> {
    let signature = std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE")?;
    let candidates = [
        std::env::var_os("XDG_RUNTIME_DIR").map(|runtime| PathBuf::from(runtime).join("hypr")),
        // Where Hyprland put them before 0.40.
        Some(PathBuf::from("/tmp/hypr")),
    ];
    candidates
        .into_iter()
        .flatten()
        .map(|base| base.join(&signature))
        .find(|directory| directory.join(".socket.sock").exists())
}

/// One request on the query socket. Hyprland answers and closes.
fn request(directory: &Path, command: &str) -> Option<String> {
    let mut stream = UnixStream::connect(directory.join(".socket.sock")).ok()?;
    stream.set_read_timeout(Some(IO_TIMEOUT)).ok()?;
    stream.set_write_timeout(Some(IO_TIMEOUT)).ok()?;
    stream.write_all(command.as_bytes()).ok()?;
    stream.flush().ok()?;
    let mut response = String::new();
    // Bounded: the timeout is per read, so a peer that trickles bytes forever
    // would otherwise be answered with unbounded memory.
    stream
        .take(MAX_RESPONSE)
        .read_to_string(&mut response)
        .ok()?;
    Some(response)
}

#[cfg(test)]
mod tests {
    use super::*;

    const CLIENTS: &str = r#"[
        {"address": "0x5643b0ca5310", "pid": 100, "workspace": {"id": 3, "name": "3"}},
        {"address": "0x5643b0cb1810", "pid": 200, "workspace": {"id": 1, "name": "1"}},
        {"address": "0x5643b1a3eaf0", "pid": 300, "workspace": {"id": 5, "name": "special:magic"}}
    ]"#;

    #[test]
    fn the_nearest_ancestor_that_is_a_window_wins() {
        // A terminal inside a terminal: the inner one hosts us.
        let window = window_for_ancestry(CLIENTS, &[999, 300, 200, 100]).unwrap();
        assert_eq!(window.address, "5643b1a3eaf0");
        assert_eq!(window.workspace.as_deref(), Some("special:magic"));
    }

    #[test]
    fn addresses_lose_the_prefix_the_query_socket_adds() {
        // Events name the same window without `0x`; one form has to win.
        let window = window_for_ancestry(CLIENTS, &[200]).unwrap();
        assert_eq!(window.address, "5643b0cb1810");
    }

    #[test]
    fn an_ancestry_that_matches_nothing_is_no_window() {
        assert_eq!(window_for_ancestry(CLIENTS, &[7, 8, 9]), None);
    }

    #[test]
    fn a_process_owning_several_windows_is_ambiguous() {
        // A browser, or a terminal that keeps every window in one process:
        // there is no way to tell which one we are in.
        let clients = r#"[
            {"address": "0xaaa", "pid": 100, "workspace": {"id": 1, "name": "1"}},
            {"address": "0xbbb", "pid": 100, "workspace": {"id": 2, "name": "2"}}
        ]"#;
        assert_eq!(window_for_ancestry(clients, &[100]), None);
    }

    #[test]
    fn nonsense_from_the_socket_is_not_a_window() {
        assert_eq!(window_for_ancestry("not json", &[100]), None);
        assert_eq!(window_for_ancestry("{}", &[100]), None);
    }

    #[test]
    fn a_window_without_a_workspace_still_locates_it() {
        let clients = r#"[{"address": "0xaaa", "pid": 100}]"#;
        let window = window_for_ancestry(clients, &[100]).unwrap();
        assert_eq!(window.workspace, None);
    }

    #[test]
    fn a_move_names_the_window_and_its_new_workspace() {
        assert_eq!(
            parse_event("movewindow>>5643b0cb1810,3"),
            Some(Event::Moved {
                address: "5643b0cb1810".into(),
                workspace: "3".into(),
            })
        );
    }

    #[test]
    fn the_v2_move_drops_the_workspace_id() {
        assert_eq!(
            parse_event("movewindowv2>>5643b0cb1810,5,special:magic"),
            Some(Event::Moved {
                address: "5643b0cb1810".into(),
                workspace: "special:magic".into(),
            })
        );
    }

    #[test]
    fn a_workspace_name_may_contain_the_separator() {
        assert_eq!(
            parse_event("movewindowv2>>aaa,5,a,b"),
            Some(Event::Moved {
                address: "aaa".into(),
                workspace: "a,b".into(),
            })
        );
    }

    #[test]
    fn a_close_names_the_window() {
        assert_eq!(
            parse_event("closewindow>>5643b0cb1810"),
            Some(Event::Closed {
                address: "5643b0cb1810".into()
            })
        );
    }

    #[test]
    fn the_noisy_events_are_ignored() {
        // Titles change on every spinner frame; this is the common case.
        assert_eq!(parse_event("windowtitle>>5643b0cb1810"), None);
        assert_eq!(parse_event("activewindowv2>>5643b0cb1810"), None);
        assert_eq!(parse_event("workspace>>3"), None);
        assert_eq!(parse_event("garbage"), None);
    }

    #[test]
    fn the_parent_pid_survives_a_command_name_with_spaces() {
        // `comm` is arbitrary bytes in parentheses — splitting from the left
        // would read the wrong field.
        assert_eq!(
            parse_ppid("1234 (my program (x)) S 42 1234 1234 0 -1 4194304"),
            Some(42)
        );
    }

    #[test]
    fn a_stat_line_that_makes_no_sense_has_no_parent() {
        assert_eq!(parse_ppid("garbage"), None);
        assert_eq!(parse_ppid("1234 (sh) S"), None);
    }
}
