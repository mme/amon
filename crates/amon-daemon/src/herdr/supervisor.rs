//! One thread per attached herdr session, projecting its agents into the
//! registry for exactly as long as the session has a client (spec decision:
//! no client, no rows).
//!
//! Structure over cleverness: any structural event — an agent appearing or
//! leaving, a pane closing or moving — tears the connection down and starts
//! over with a fresh snapshot. Those are rare; re-reading the world costs a
//! few socket round trips and cannot drift. Only status changes, the common
//! case, are handled in-stream, as patches keyed to the entry's connection,
//! so the registry's sound machinery sees the crossings the way it does for
//! wrapped agents.

use std::collections::HashMap;
use std::net::Shutdown;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use amon_protocol::{AgentEntry, AgentPatch};

use super::proc::{self, HerdrSession};
use super::{project, wire};
use crate::Registry;

const RESCAN_INTERVAL: Duration = Duration::from_secs(2);
const RETRY_INTERVAL: Duration = Duration::from_secs(1);

/// Starts watching this machine for herdr sessions, forever.
///
/// `connections` is the daemon's connection-id counter: projected entries
/// and real socket connections draw from one id space, because the registry
/// keys ownership by connection id and two allocators would collide.
pub fn spawn(registry: Registry, connections: Arc<AtomicU64>) {
    std::thread::spawn(move || watch(registry, connections));
}

struct Session {
    stop: Arc<AtomicBool>,
    /// Set by the session thread once it has let go of every entry it owned.
    done: Arc<AtomicBool>,
    client_pid: u32,
}

/// How long a replacement waits for the session it replaces to let go.
/// Bounded, because a wedged supervisor must not stop the watcher: the
/// worst case then is the race this exists to avoid, which is recoverable,
/// while a stuck watcher is not.
const HANDOVER_GRACE: Duration = Duration::from_secs(2);

fn watch(registry: Registry, connections: Arc<AtomicU64>) {
    // Keyed by socket, which is what a session *is*: two clients with no
    // session name pointed at different sockets are two sessions, and the
    // name is only what the rows are called.
    let mut live: HashMap<PathBuf, Session> = HashMap::new();
    loop {
        let found: HashMap<PathBuf, HerdrSession> = proc::sessions()
            .into_iter()
            .map(|session| (session.socket.clone(), session))
            .collect();

        // A session whose client is gone — or replaced by a different one —
        // stops showing; a replacement client starts fresh below, window and
        // all.
        live.retain(|socket, session| {
            let keep = found
                .get(socket)
                .is_some_and(|now| now.client_pid == session.client_pid);
            if !keep {
                session.stop.store(true, Ordering::Relaxed);
                // Waited for here, before anyone takes its place. A stopped
                // supervisor still between its last stop check and its
                // reconcile would otherwise take the replacement's claims
                // for its own and then disconnect them on its way out,
                // leaving the session with no rows at all until something
                // structural happens — which, for agents at rest, is never.
                await_done(&session.done);
            }
            keep
        });

        for (socket, session) in found {
            if live.contains_key(&socket) {
                continue;
            }
            let stop = Arc::new(AtomicBool::new(false));
            let done = Arc::new(AtomicBool::new(false));
            live.insert(
                socket,
                Session {
                    stop: stop.clone(),
                    done: done.clone(),
                    client_pid: session.client_pid,
                },
            );
            let registry = registry.clone();
            let connections = connections.clone();
            std::thread::spawn(move || {
                let watch = WindowWatch::open(session.client_pid);
                run_session(session, registry, connections, stop, done, watch);
            });
        }
        std::thread::sleep(RESCAN_INTERVAL);
    }
}

/// The compositor's answer about the client's window, and the stream that
/// keeps it current.
pub(crate) struct WindowWatch {
    directory: PathBuf,
    events: UnixStream,
    window: Option<amon_hypr::Window>,
}

impl WindowWatch {
    /// Subscribes, *then* looks the window up.
    ///
    /// That order is the whole point, and it is the wrapper's order too: a
    /// move between the lookup and the subscription would otherwise be lost,
    /// and the lookup is not instant — it waits for the window to be mapped,
    /// which for a terminal started with a command in it takes a moment.
    /// Events queue from the moment of connection, so the worst case is a
    /// snapshot briefly behind rather than a workspace wrong until the next
    /// time the window happens to move.
    fn open(pid: u32) -> Option<Self> {
        let directory = compositor_dir(pid)?;
        let events = amon_hypr::connect_events(&directory).ok()?;
        let window = amon_hypr::resolve_when_mapped(&directory, pid);
        Some(Self {
            directory,
            events,
            window,
        })
    }
}

/// The compositor the *client* is talking to, not the one the daemon was
/// started from.
///
/// They are usually the same and occasionally not: a daemon started over
/// ssh, or by a user service, carries no compositor at all, and would then
/// find no window for any herdr client while wrapped agents — which each ask
/// from inside the session — keep resolving theirs. Asking on the client's
/// behalf keeps the two paths equally good.
fn compositor_dir(pid: u32) -> Option<PathBuf> {
    let environ = std::fs::read(format!("/proc/{pid}/environ")).unwrap_or_default();
    let environ = String::from_utf8_lossy(&environ);
    let Some(signature) = proc::env_value(&environ, "HYPRLAND_INSTANCE_SIGNATURE") else {
        // An unreadable environ leaves the daemon's own compositor as the
        // best guess, and on one desktop it is the right one.
        return amon_hypr::socket_dir();
    };
    let runtime = proc::env_value(&environ, "XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .or_else(amon_hypr::runtime_dir);
    amon_hypr::socket_dir_for(std::ffi::OsStr::new(signature), runtime)
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or(0)
}

/// What the session thread and the window follower both touch.
struct Shared {
    window: Option<amon_hypr::Window>,
    /// terminal_id → its registry claim. Keyed by herdr's stable terminal
    /// identity, not by pane id: a pane move renames the pane but must not
    /// look like an agent leaving and another arriving.
    owned: HashMap<String, Owned>,
}

struct Owned {
    connection: u64,
    /// The entry as last registered, kept so updates can preserve
    /// `state_since` across resnapshots and skip no-op registrations.
    entry: AgentEntry,
    /// herdr's transition counter as of that entry, which is how a restart
    /// in the same terminal is told from the life already being watched.
    seq: Option<u64>,
}

/// Whether the terminal's agent counter went backwards, which only a new
/// agent can do. Unknown on either side is not evidence of anything —
/// herdr may not have said — so it reads as the same life, leaving the
/// agent label as the only distinction, exactly as before.
fn restarted(was: Option<u64>, now: Option<u64>) -> bool {
    matches!((was, now), (Some(was), Some(now)) if now < was)
}

/// Waits, bounded, for a stopped session to finish letting go.
fn await_done(done: &Arc<AtomicBool>) {
    let deadline = std::time::Instant::now() + HANDOVER_GRACE;
    while !done.load(Ordering::Relaxed) && std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
}

pub(crate) fn run_session(
    session: HerdrSession,
    registry: Registry,
    connections: Arc<AtomicU64>,
    stop: Arc<AtomicBool>,
    done: Arc<AtomicBool>,
    watch: Option<WindowWatch>,
) {
    let shared = Arc::new(Mutex::new(Shared {
        window: watch.as_ref().and_then(|watch| watch.window.clone()),
        owned: HashMap::new(),
    }));
    let current = Arc::new(Mutex::new(None::<wire::ShutdownHandle>));
    // A second handle on the compositor stream, so stopping can close it out
    // from under the follower — which is otherwise parked in a blocking read
    // and would outlive the session it belongs to.
    let compositor = watch
        .as_ref()
        .and_then(|watch| watch.events.try_clone().ok());
    spawn_stop_watch(&stop, &done, &current, compositor);
    spawn_window_follow(watch, session.client_pid, &registry, &shared, &stop);

    while !stop.load(Ordering::Relaxed) {
        let panes: Vec<String> = lock(&shared)
            .owned
            .values()
            .filter_map(|owned| owned.entry.herdr.as_ref().map(|h| h.pane.clone()))
            .collect();
        let Ok(events) = wire::subscribe(
            &session.socket,
            subscriptions_for(panes.iter().map(String::as_str)),
        ) else {
            std::thread::sleep(RETRY_INTERVAL);
            continue;
        };
        *lock(&current) = Some(events.shutdown_handle());

        let Ok(snapshot) =
            wire::request(&session.socket, "session.snapshot", serde_json::json!({}))
        else {
            std::thread::sleep(RETRY_INTERVAL);
            continue;
        };
        // The client may have gone while that request was in flight. Writing
        // this snapshot now would put a window that has closed back on rows
        // the replacement session has already corrected — and the correction
        // will not come again, because this session's own disconnect is
        // suppressed once the replacement holds the same ids.
        if stop.load(Ordering::Relaxed) {
            break;
        }
        reconcile(
            &wire::agents_from(&snapshot),
            &session,
            &registry,
            &connections,
            &shared,
        );
        // The subscription covers the agent panes known *before* this
        // snapshot. If the set changed, resubscribe before trusting it —
        // a status change on a pane outside the set would go unheard.
        let mut current_panes: Vec<String> = lock(&shared)
            .owned
            .values()
            .filter_map(|owned| owned.entry.herdr.as_ref().map(|h| h.pane.clone()))
            .collect();
        let mut subscribed = panes;
        current_panes.sort();
        subscribed.sort();
        if current_panes != subscribed {
            continue;
        }

        for event in events {
            if stop.load(Ordering::Relaxed) {
                break;
            }
            if event.event != "pane.agent_status_changed" {
                // Structural: start over with a fresh snapshot.
                break;
            }
            apply_status(&registry, &shared, &event.data);
        }
    }

    for owned in lock(&shared).owned.drain().map(|(_, owned)| owned) {
        registry.disconnect(owned.connection);
    }
    done.store(true, Ordering::Relaxed);
}

fn lock<T>(shared: &Arc<Mutex<T>>) -> std::sync::MutexGuard<'_, T> {
    shared
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Global lifecycle events, plus one status subscription per known agent
/// pane — herdr has no global status subscription, and a connection's set is
/// fixed, which is why structural changes reconnect.
fn subscriptions_for<'a>(panes: impl Iterator<Item = &'a str>) -> Vec<serde_json::Value> {
    let mut subscriptions = vec![
        serde_json::json!({"type": "pane.agent_detected"}),
        serde_json::json!({"type": "pane.closed"}),
        serde_json::json!({"type": "pane.exited"}),
        serde_json::json!({"type": "pane.moved"}),
    ];
    subscriptions.extend(
        panes.map(|pane| serde_json::json!({"type": "pane.agent_status_changed", "pane_id": pane})),
    );
    subscriptions
}

fn reconcile(
    agents: &[wire::HerdrAgent],
    session: &HerdrSession,
    registry: &Registry,
    connections: &Arc<AtomicU64>,
    shared: &Arc<Mutex<Shared>>,
) {
    let mut shared = lock(shared);
    let window = shared.window.clone();
    let mut present: std::collections::HashSet<String> = Default::default();
    for agent in agents {
        present.insert(agent.terminal_id.clone());
        let mut entry = project::entry_for(agent, session, window.as_ref(), now_ms());
        let seq = agent.state_change_seq;
        let previous = shared
            .owned
            .get(&agent.terminal_id)
            .map(|owned| (owned.connection, owned.entry.clone(), owned.seq));

        // A resnapshot is a fresh look at the same agent, not a fresh agent,
        // so its clocks carry over — but only while it *is* the same agent.
        // A herdr terminal outlives what runs in it: finish with claude and
        // start codex, or just claude again, and the terminal id is
        // unchanged while the lifetime is not. Carrying clocks across that
        // would date the new agent from the old one's launch and sort it by
        // a wait it never had.
        let same_lifetime = previous
            .as_ref()
            .is_some_and(|(_, was, was_seq)| was.agent == entry.agent && !restarted(*was_seq, seq));

        if let (true, Some((connection, was, _))) = (same_lifetime, previous.clone()) {
            entry.started_at = was.started_at;
            // Same rule as the event path: the clock belongs to the state,
            // and being seen does not restart it.
            if entry.state == was.state {
                entry.state_since = was.state_since;
            }
            if entry != was {
                registry.register(connection, entry.clone());
            }
            shared.owned.insert(
                agent.terminal_id.clone(),
                Owned {
                    connection,
                    entry,
                    seq,
                },
            );
            continue;
        }

        // A new lifetime in this terminal. The old claim is retired first so
        // the new one arrives as what it is — a first claim, which by rule
        // rings for nothing. Registering the newcomer over the old claim
        // instead would have the registry read one agent's state against
        // another's and chime for a crossing neither of them made.
        if let Some((old, _, _)) = previous {
            registry.disconnect(old);
        }
        let connection = connections.fetch_add(1, Ordering::Relaxed);
        registry.register(connection, entry.clone());
        shared.owned.insert(
            agent.terminal_id.clone(),
            Owned {
                connection,
                entry,
                seq,
            },
        );
    }
    shared.owned.retain(|_, owned| {
        let keep = owned
            .entry
            .id
            .rsplit(':')
            .next()
            .is_some_and(|terminal| present.contains(terminal));
        if !keep {
            registry.disconnect(owned.connection);
        }
        keep
    });
}

/// Applies one `pane.agent_status_changed`.
///
/// The event carries the agent's presentation as well as its status, and
/// they move independently: a working agent that starts on something else
/// keeps its status and changes its title. Applying only the status would
/// leave the row describing work that finished a while ago, so both travel
/// in one patch — and the clock only restarts when the state itself moved,
/// or every title change would reset how long the agent has been at it.
fn apply_status(registry: &Registry, shared: &Arc<Mutex<Shared>>, data: &serde_json::Value) {
    let Some(pane) = data.get("pane_id").and_then(|id| id.as_str()) else {
        return;
    };
    let Some(status) = data.get("agent_status").and_then(|status| status.as_str()) else {
        return;
    };
    let mut shared = lock(shared);
    let Some(owned) = shared
        .owned
        .values_mut()
        .find(|owned| owned.entry.herdr.as_ref().is_some_and(|h| h.pane == pane))
    else {
        return;
    };

    let mut patch = AgentPatch::new(owned.entry.id.clone());
    let (state, seen) = project::state_for(status);
    // `state_since` is when the agent entered this state, and being looked
    // at is not entering anything. herdr's `done` and `idle` are one amon
    // state either side of `seen`, so focusing a finished agent arrives here
    // as a seen-only change — and restarting the clock for it would turn
    // "finished twenty minutes ago" into "finished just now" for no reason
    // but that you glanced at it. Wrapped agents keep their clock across the
    // same moment.
    if owned.entry.state != state {
        let since = now_ms();
        patch.state = Some(state);
        patch.state_since = Some(since);
        owned.entry.state = state;
        owned.entry.state_since = since;
    }
    if owned.entry.seen != seen {
        patch.seen = Some(seen);
        owned.entry.seen = seen;
    }
    // Present-and-null means herdr no longer has a name for what is in the
    // pane; absent means it did not say. The projection reads a null the
    // same way, and the two paths must agree or the row keeps the name of an
    // agent that has gone.
    let label = match data.get("agent") {
        None => None,
        Some(serde_json::Value::Null) => Some(project::UNNAMED.to_owned()),
        Some(agent) => agent.as_str().map(str::to_owned),
    };
    if let Some(label) = label {
        if owned.entry.agent != label {
            patch.agent = Some(label.clone());
            owned.entry.agent = label;
        }
    }
    // Present-and-null clears the title; absent leaves it alone.
    let title = match data.get("title") {
        None => None,
        Some(serde_json::Value::Null) => Some(None),
        Some(title) => title.as_str().map(|title| Some(title.to_owned())),
    };
    if let Some(title) = title {
        if owned.entry.title != title {
            patch.title = Some(title.clone());
            owned.entry.title = title;
        }
    }

    if patch == AgentPatch::new(owned.entry.id.clone()) {
        return;
    }
    registry.update(owned.connection, &patch);
}

/// Ends the blocking reads when the session is asked to stop — neither
/// reader can poll a flag while parked in `read_line`. Keeps sweeping until
/// the session thread confirms it is done, so a herdr stream raced into
/// place just as the stop landed is shut down too.
///
/// Both streams matter. The herdr one ends the session loop; the
/// compositor one ends the window follower, which is otherwise blocked on a
/// socket that stays open for as long as Hyprland runs — one leaked thread
/// and file descriptor per detach, on a daemon that is meant to run for
/// weeks.
fn spawn_stop_watch(
    stop: &Arc<AtomicBool>,
    done: &Arc<AtomicBool>,
    current: &Arc<Mutex<Option<wire::ShutdownHandle>>>,
    compositor: Option<UnixStream>,
) {
    let stop = stop.clone();
    let done = done.clone();
    let current = current.clone();
    std::thread::spawn(move || {
        while !done.load(Ordering::Relaxed) && !stop.load(Ordering::Relaxed) {
            std::thread::sleep(Duration::from_millis(100));
        }
        // Sweep until the session confirms it has finished: it may open one
        // more herdr stream between the stop landing and the loop noticing.
        while !done.load(Ordering::Relaxed) {
            if let Some(handle) = lock(&current).take() {
                handle.shutdown();
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        if let Some(handle) = lock(&current).take() {
            handle.shutdown();
        }
        if let Some(compositor) = compositor {
            let _ = compositor.shutdown(Shutdown::Both);
        }
    });
}

/// Follows the client's window between Hyprland workspaces, patching every
/// entry the session owns. The agents all live in one window — the herdr
/// client's terminal — so one move patches them all.
fn spawn_window_follow(
    watch: Option<WindowWatch>,
    pid: u32,
    registry: &Registry,
    shared: &Arc<Mutex<Shared>>,
    stop: &Arc<AtomicBool>,
) {
    // No compositor, or a client the compositor never placed: nothing to
    // follow, and the stream (if any) closes with the watch.
    let Some(watch) = watch else {
        return;
    };
    let Some(start) = watch.window.clone() else {
        return;
    };
    let (directory, events) = (watch.directory, watch.events);
    let registry = registry.clone();
    let shared = shared.clone();
    let stop = stop.clone();
    std::thread::spawn(move || {
        amon_hypr::follow(&directory, events, pid, start, |window| {
            if stop.load(Ordering::Relaxed) {
                return;
            }
            let mut shared = lock(&shared);
            shared.window = window.clone();
            for owned in shared.owned.values_mut() {
                let mut patch = AgentPatch::new(owned.entry.id.clone());
                patch.window = Some(window.as_ref().map(|w| w.address.clone()));
                patch.workspace = Some(window.as_ref().and_then(|w| w.workspace.clone()));
                registry.update(owned.connection, &patch);
                owned.entry.window = window.as_ref().map(|w| w.address.clone());
                owned.entry.workspace = window.as_ref().and_then(|w| w.workspace.clone());
            }
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::sync::atomic::AtomicU64;
    use std::sync::mpsc;
    use std::time::Duration;

    use amon_protocol::AgentState;

    /// A canned herdr: answers `session.snapshot` with the given agents,
    /// acks subscriptions and keeps them open, and fans every line pushed
    /// through the returned sender out to all live subscriptions.
    fn fake_herdr(
        agents: serde_json::Value,
    ) -> (tempfile::TempDir, std::path::PathBuf, mpsc::Sender<String>) {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("herdr.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let (pushes, inbox) = mpsc::channel::<String>();
        let subscribers: Arc<Mutex<Vec<UnixStream>>> = Arc::default();

        let fanout = subscribers.clone();
        std::thread::spawn(move || {
            while let Ok(line) = inbox.recv() {
                fanout
                    .lock()
                    .unwrap()
                    .retain_mut(|stream| writeln!(stream, "{line}").is_ok());
            }
        });

        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap_or(0) == 0 {
                    continue;
                }
                let mut stream = stream;
                if line.contains("events.subscribe") {
                    let _ = writeln!(
                        stream,
                        r#"{{"id":"s","result":{{"type":"subscription_started"}}}}"#
                    );
                    subscribers.lock().unwrap().push(stream);
                } else if line.contains("session.snapshot") {
                    let result = serde_json::json!({
                        "id": "r",
                        "result": {"type": "session_snapshot", "agents": agents},
                    });
                    let _ = writeln!(stream, "{result}");
                }
            }
        });
        (dir, socket, pushes)
    }

    fn wait_for<T>(mut probe: impl FnMut() -> Option<T>) -> T {
        for _ in 0..150 {
            if let Some(value) = probe() {
                return value;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        panic!("condition not reached within 3s");
    }

    #[test]
    fn a_terminal_outlives_its_agents_and_each_gets_its_own_clock() {
        // herdr's terminal id belongs to the pane, not to what runs in it.
        // Only a counter going backwards, or a different agent, marks the
        // moment one life ended and the next began.
        assert!(!restarted(None, None), "herdr said nothing either time");
        assert!(!restarted(Some(4), None), "and nothing is not evidence");
        assert!(!restarted(Some(4), Some(4)), "standing still is one life");
        assert!(!restarted(Some(4), Some(9)), "counting up is one life");
        assert!(restarted(Some(9), Some(1)), "only a new agent counts down");
    }

    #[test]
    fn a_session_registers_its_agents_patches_status_and_deregisters_on_stop() {
        let (_dir, socket, pushes) = fake_herdr(serde_json::json!([
            {"terminal_id": "t1", "agent": "claude", "agent_status": "working",
             "pane_id": "w1:p1", "cwd": "/repo"}
        ]));
        let registry = crate::Registry::new();
        let stop = Arc::new(AtomicBool::new(false));
        let session = crate::herdr::proc::HerdrSession {
            name: Some("work".into()),
            socket,
            client_pid: std::process::id(),
        };
        let handle = std::thread::spawn({
            let registry = registry.clone();
            let stop = stop.clone();
            move || {
                run_session(
                    session,
                    registry,
                    Arc::new(AtomicU64::new(1000)),
                    stop,
                    Arc::new(AtomicBool::new(false)),
                    None,
                )
            }
        });

        // The snapshot lands as a registered entry.
        let entry = wait_for(|| {
            registry
                .agents()
                .into_iter()
                .find(|agent| agent.id.ends_with(":t1"))
        });
        assert_eq!(entry.state, AgentState::Working);
        assert_eq!(entry.herdr.as_ref().unwrap().pane, "w1:p1");

        // A status event lands as a patch. Sent until observed: the
        // supervisor may still be between its first and second subscription,
        // and a repeat of the same state is a no-op by design.
        wait_for(|| {
            let event = r#"{"event":"pane.agent_status_changed","data":{"pane_id":"w1:p1","agent_status":"blocked"}}"#;
            let _ = pushes.send(event.into());
            registry
                .agents()
                .into_iter()
                .find(|agent| agent.id.ends_with(":t1") && agent.state == AgentState::Blocked)
        });

        // Presentation travels on the same event, and moves on its own: an
        // agent that stays blocked while its title changes must not go on
        // describing what it was doing before.
        let settled = wait_for(|| {
            let event = r#"{"event":"pane.agent_status_changed","data":{"pane_id":"w1:p1","agent_status":"blocked","title":"waiting on approval","agent":"claude-code"}}"#;
            let _ = pushes.send(event.into());
            registry
                .agents()
                .into_iter()
                .find(|agent| agent.title.as_deref() == Some("waiting on approval"))
        });
        assert_eq!(settled.agent, "claude-code");
        assert_eq!(settled.state, AgentState::Blocked, "the state held still");

        // Stop tears every entry down.
        stop.store(true, Ordering::Relaxed);
        handle.join().unwrap();
        assert!(registry.agents().is_empty());
    }
}
