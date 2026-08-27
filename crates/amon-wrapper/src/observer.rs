//! The one thread that owns what amon knows about the agent.
//!
//! Screen signals and hook reports both feed the vendored `TerminalState`,
//! which arbitrates them into an effective state (ADR-0001). Keeping that
//! state, the shadow terminal, and detection on a single thread means no locks
//! and no chance of the two signal sources racing each other — everything
//! arrives as a message and is handled in order.
//!
//! This thread is never in the path of the agent's output: the reader writes
//! to the real terminal first and only then sends a copy here, so a slow
//! detection pass can never delay what the user sees.

use std::path::PathBuf;
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

use amon_detect::{detect_agent_with_osc, Agent};
use amon_protocol::{AgentEntry, AgentPatch, AgentState};
use amon_term::{ShadowTerminal, TerminalId, TerminalState};

use crate::daemon_link::DaemonLink;

/// How often detection runs at most. Agents repaint far more often than this,
/// and a spinner frame does not need to be classified twice.
const DETECTION_INTERVAL: Duration = Duration::from_millis(150);

/// How long a remote agent may go unheard before its row reverts. The remote
/// wrapper whispers at least as often as its daemon link wakes — every 30
/// seconds at the deepest reconnect backoff — so three missed wakes means the
/// far side is gone, not slow.
const WHISPER_TIMEOUT: Duration = Duration::from_secs(90);

pub enum Signal {
    /// A copy of what the agent just wrote to the terminal.
    Output(Vec<u8>),
    Resize {
        cols: u16,
        rows: u16,
    },
    /// An integration hook reported state.
    HookState {
        source: String,
        agent: String,
        state: AgentState,
        seq: Option<u64>,
        /// Hooks stamp the session id on state reports too, so identity
        /// survives even when the one-shot session report was missed.
        session_id: Option<String>,
    },
    /// The terminal reported that the agent's view gained or lost focus.
    Focus(bool),
    /// The compositor placed the agent's window, or moved it.
    Window {
        window: Option<String>,
        workspace: Option<String>,
    },
    /// An integration hook reported which session the agent is in.
    HookSession {
        source: String,
        agent: String,
        session_id: String,
        session_path: Option<String>,
        seq: Option<u64>,
        /// `startup`, `resume`, `new`, … — what began this session, when the
        /// hook knows.
        session_start_source: Option<String>,
    },
    /// The branch the agent's directory is on changed, or first became known.
    /// `None` means it is on no branch: outside a repository, or detached.
    Branch(Option<String>),
    /// A whispered report from a wrapper on the far side of this session's
    /// stream: a remote agent claiming this row.
    Remote(RemoteReport),
    AgentExited,
}

/// Registry traffic that arrived as a whisper, plus the goodbye.
pub enum RemoteReport {
    Register(Box<AgentEntry>),
    Update(Box<AgentPatch>),
    Bye,
}

pub struct Observer {
    shadow: ShadowTerminal,
    state: TerminalState,
    agent: Option<Agent>,
    agent_id: String,
    link: DaemonLink,
    last_reported: AgentState,
    last_title: Option<String>,
    dirty: bool,
    last_detection: Instant,
    /// Newest identity-carrying report sequence seen per hook source. Tracked
    /// separately from arbitration: the state machine may buffer a valid
    /// report (also returning `None`), so its verdict cannot tell a stale
    /// report from a merely deferred one. Only reports that carry a session
    /// id advance this — an identity-less state report must not starve a
    /// later-arriving session report.
    identity_seqs: std::collections::HashMap<String, u64>,
    /// The session id last patched into the registry. A session report for
    /// this same session may always add metadata (the path), whatever its
    /// sequence — only a *different* session needs to prove it is newer.
    last_session_id: Option<String>,
    focus: crate::focus::Tracker,
    /// The wrapper's own registry entry, kept current by applying every own
    /// patch to it — sent or suppressed — so that a revert from a remote
    /// agent re-registers today's facts, not launch-time's.
    own_entry: AgentEntry,
    /// The compositor's placement, stored as well as forwarded: a remote
    /// agent taking the row inherits the window it is being watched through.
    window: Option<String>,
    workspace: Option<String>,
    /// Present while a whispered remote agent holds the row.
    remote: Option<Mirror>,
}

/// What the observer tracks about the remote agent on its row.
struct Mirror {
    last_heard: Instant,
    /// The mirrored state and when it began — locally stamped, so a
    /// heartbeat repeating the same state cannot reset the clock and remote
    /// clock skew never shows up in a duration.
    state: AgentState,
    state_since: u64,
    started_at: u64,
}

/// What the observer needs to know about the agent it is watching.
pub struct Setup {
    pub agent_id: String,
    pub agent: Option<Agent>,
    pub cwd: PathBuf,
    pub cols: u16,
    pub rows: u16,
    /// The entry the wrapper registered, as it registered it.
    pub entry: AgentEntry,
}

/// Starts the observer on its own thread.
///
/// The shadow terminal holds raw pointers into Ghostty and is not `Send`, so
/// it is built here rather than handed over — which also means a terminal that
/// cannot be created costs the agent nothing: observation stops, the agent
/// runs on.
pub fn spawn(
    setup: Setup,
    link: DaemonLink,
    signals: Receiver<Signal>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || match Observer::new(setup, link) {
        Ok(observer) => observer.run(signals),
        Err(error) => {
            eprintln!("amon: shadow terminal unavailable, state will not be reported: {error}");
        }
    })
}

impl Observer {
    fn new(setup: Setup, link: DaemonLink) -> Result<Self, libghostty_vt::Error> {
        Ok(Self {
            shadow: ShadowTerminal::new(setup.cols, setup.rows)?,
            state: TerminalState::new(TerminalId::alloc(), setup.cwd),
            agent: setup.agent,
            agent_id: setup.agent_id,
            link,
            last_reported: AgentState::Unknown,
            last_title: None,
            dirty: false,
            last_detection: Instant::now(),
            identity_seqs: std::collections::HashMap::new(),
            last_session_id: None,
            focus: crate::focus::Tracker::default(),
            own_entry: setup.entry,
            window: None,
            workspace: None,
            remote: None,
        })
    }

    /// Runs until the channel closes, which happens when the wrapper drops its
    /// sender on the way out.
    fn run(mut self, signals: Receiver<Signal>) {
        loop {
            match signals.recv_timeout(DETECTION_INTERVAL) {
                Ok(signal) => {
                    let exited = matches!(signal, Signal::AgentExited);
                    self.handle(signal);
                    if exited {
                        self.detect(true);
                        return;
                    }
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return,
            }

            // A quiet terminal still needs classifying: going idle is the
            // absence of output, not an event.
            if self.dirty && self.last_detection.elapsed() >= DETECTION_INTERVAL {
                self.detect(false);
            }

            // A remote agent that stopped whispering is gone — its host
            // unreachable, its wrapper killed — and the row reverts to what
            // this wrapper can still vouch for itself.
            self.check_remote_liveness();
        }
    }

    fn check_remote_liveness(&mut self) {
        if let Some(mirror) = &self.remote {
            if mirror.last_heard.elapsed() > WHISPER_TIMEOUT {
                self.revert();
            }
        }
    }

    /// Sends a patch about the wrapper's own agent — or, while a remote agent
    /// holds the row, only remembers it, so a revert re-registers current
    /// facts rather than stale ones.
    fn own_update(&mut self, patch: AgentPatch) {
        patch.apply(&mut self.own_entry);
        if self.remote.is_none() {
            self.link.update(patch);
        }
    }

    fn handle_remote(&mut self, report: RemoteReport) {
        match report {
            RemoteReport::Register(remote) => {
                let now = crate::now_millis();
                // Locally stamped clocks: fresh only when the state actually
                // changed, so a heartbeat repeating the same state keeps its
                // duration and remote clock skew never reaches a consumer.
                let (state_since, started_at) = match &self.remote {
                    Some(mirror) if mirror.state == remote.state => {
                        (mirror.state_since, mirror.started_at)
                    }
                    Some(mirror) => (now, mirror.started_at),
                    None => (now, now),
                };
                let merged = AgentEntry {
                    id: self.own_entry.id.clone(),
                    window: self.window.clone(),
                    workspace: self.workspace.clone(),
                    state_since,
                    started_at,
                    ..(*remote)
                };
                self.remote = Some(Mirror {
                    last_heard: Instant::now(),
                    state: merged.state,
                    state_since,
                    started_at,
                });
                self.link.register(merged);
            }
            RemoteReport::Update(patch) => {
                // An update can only follow a register; one without is a
                // stream this build cannot anchor, and costs nothing.
                let Some(mirror) = self.remote.as_mut() else {
                    return;
                };
                mirror.last_heard = Instant::now();
                let mut patch = *patch;
                patch.id = self.own_entry.id.clone();
                // The compositor on this side owns placement, whoever owns
                // the row.
                patch.window = None;
                patch.workspace = None;
                match patch.state {
                    Some(state) if state != mirror.state => {
                        mirror.state = state;
                        mirror.state_since = crate::now_millis();
                        patch.state_since = Some(mirror.state_since);
                    }
                    _ => patch.state_since = None,
                }
                self.link.update(patch);
            }
            RemoteReport::Bye => self.revert(),
        }
    }

    /// The remote agent is gone; the row is the wrapper's own again.
    fn revert(&mut self) {
        if self.remote.take().is_some() {
            self.link.register(self.own_entry.clone());
        }
    }

    fn handle(&mut self, signal: Signal) {
        match signal {
            Signal::Output(bytes) => {
                self.shadow.feed(&bytes);
                self.dirty = true;
            }
            Signal::Focus(focused) => {
                if self.focus.focus_changed(focused) {
                    let mut patch = AgentPatch::new(&self.agent_id);
                    patch.focused = Some(self.focus.focused());
                    patch.seen = Some(self.focus.seen());
                    // A remote agent computes its own focus and seen from the
                    // same reports, paired with its own state transitions.
                    self.own_update(patch);
                }
            }
            Signal::Window { window, workspace } => {
                self.window.clone_from(&window);
                self.workspace.clone_from(&workspace);
                let mut patch = AgentPatch::new(&self.agent_id);
                patch.window = Some(window);
                patch.workspace = Some(workspace);
                patch.apply(&mut self.own_entry);
                // Placement is this compositor's fact and holds for whoever
                // owns the row, so it is never suppressed.
                self.link.update(patch);
            }
            Signal::Branch(branch) => {
                let mut patch = AgentPatch::new(&self.agent_id);
                patch.branch = Some(branch);
                // The branch of the directory the session was opened from —
                // not the remote agent's, which reports its own.
                self.own_update(patch);
            }
            Signal::Resize { cols, rows } => {
                let _ = self.shadow.resize(cols, rows);
                self.dirty = true;
            }
            Signal::HookState {
                source,
                agent,
                state,
                seq,
                session_id,
            } => {
                // Identity rides on state reports too: if the one-shot session
                // report was missed, the next state report heals the entry.
                // Ordered by the source's own sequence so a delayed report
                // from an older session cannot regress the registry.
                if let Some(session_id) = session_id.clone() {
                    if self.identity_report_is_fresh(&source, seq) {
                        self.last_session_id = Some(session_id.clone());
                        let mut patch = AgentPatch::new(&self.agent_id);
                        patch.agent_session_id = Some(session_id);
                        self.own_update(patch);
                    }
                }

                // The authority is scoped to the reported session, as herdr
                // scopes it, so a report from a finished session cannot
                // outvote the screen after a `--resume` or `/clear`.
                let session_ref =
                    amon_term::session_ref_from_report(&source, &agent, session_id, None);
                // `set_hook_authority_at` is the real entry point; the
                // shorter `set_hook_authority` upstream is a test convenience.
                let change = self.state.set_hook_authority_at(
                    source,
                    agent,
                    to_detect_state(state),
                    None,
                    session_ref,
                    seq,
                    Instant::now(),
                );
                if change.is_some() {
                    self.publish();
                }
            }
            Signal::HookSession {
                source,
                agent,
                session_id,
                session_path,
                seq,
                session_start_source,
            } => {
                // A dedicated session report is authoritative for identity —
                // regardless of what arbitration makes of it — but still only
                // in its source's own order, so a delayed report cannot roll
                // identity back. A report for the *current* session always
                // applies: state reports never carry the path, so this is how
                // it gets filled in even when the session report lost the race.
                let same_session = self.last_session_id.as_deref() == Some(session_id.as_str());
                if self.identity_report_is_fresh(&source, seq) || same_session {
                    self.last_session_id = Some(session_id.clone());
                    let mut patch = AgentPatch::new(&self.agent_id);
                    patch.agent = Some(agent.clone());
                    patch.agent_session_id = Some(session_id.clone());
                    patch.agent_session_path = session_path.clone();
                    self.own_update(patch);
                }

                // The state machine is reanchored to the new session, as herdr
                // does on its session-report path: authority is session-scoped,
                // so without this a mid-process session change (`--resume`, a
                // fresh session in the same process) would leave the machine
                // anchored to the old session and rejecting the new one's
                // state reports.
                let session_ref = amon_term::session_ref_from_report(
                    &source,
                    &agent,
                    Some(session_id),
                    session_path,
                );
                let change = self.state.set_agent_session_ref_for_session_start(
                    source,
                    agent,
                    session_ref,
                    seq,
                    amon_term::normalize_session_start_source(session_start_source),
                );
                if change.is_some() {
                    self.publish();
                }
            }
            Signal::Remote(report) => self.handle_remote(report),
            Signal::AgentExited => {
                self.dirty = true;
            }
        }
    }

    /// Whether a report is the newest its source has sent, recording it if so.
    /// Reports without a sequence are taken as-is.
    fn identity_report_is_fresh(&mut self, source: &str, seq: Option<u64>) -> bool {
        let Some(seq) = seq else {
            return true;
        };
        match self.identity_seqs.get_mut(source) {
            Some(latest) if *latest >= seq => false,
            Some(latest) => {
                *latest = seq;
                true
            }
            None => {
                self.identity_seqs.insert(source.to_string(), seq);
                true
            }
        }
    }

    fn detect(&mut self, process_exited: bool) {
        self.last_detection = Instant::now();
        self.dirty = false;

        let title = self.shadow.title();
        let detection = detect_agent_with_osc(
            self.agent,
            &self.shadow.detection_text(),
            title.as_deref().unwrap_or_default(),
            "",
        );

        if !detection.skip_state_update {
            self.state.set_detected_state_with_screen_signals_at(
                self.agent,
                detection.state,
                detection.visible_blocker,
                detection.visible_idle,
                detection.visible_working,
                process_exited,
                Instant::now(),
            );
        }

        if title != self.last_title {
            self.last_title = title.clone();
            let mut patch = AgentPatch::new(&self.agent_id);
            patch.title = Some(title);
            self.own_update(patch);
        }

        self.publish();
    }

    /// Reports the arbitrated state, but only when it actually changed — the
    /// daemon's subscribers should see transitions, not a heartbeat.
    fn publish(&mut self) {
        let state = from_detect_state(self.state.state);
        if state == self.last_reported {
            return;
        }
        self.last_reported = state;
        // Seen is per-state: whatever the user saw of the last state says
        // nothing about this one.
        self.focus.state_began();

        let mut patch = AgentPatch::new(&self.agent_id);
        patch.state = Some(state);
        patch.state_since = Some(crate::now_millis());
        patch.seen = Some(self.focus.seen());
        self.own_update(patch);
    }
}

fn to_detect_state(state: AgentState) -> amon_detect::AgentState {
    match state {
        AgentState::Idle => amon_detect::AgentState::Idle,
        AgentState::Working => amon_detect::AgentState::Working,
        AgentState::Blocked => amon_detect::AgentState::Blocked,
        AgentState::Unknown => amon_detect::AgentState::Unknown,
    }
}

fn from_detect_state(state: amon_detect::AgentState) -> AgentState {
    match state {
        amon_detect::AgentState::Idle => AgentState::Idle,
        amon_detect::AgentState::Working => AgentState::Working,
        amon_detect::AgentState::Blocked => AgentState::Blocked,
        amon_detect::AgentState::Unknown => AgentState::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::daemon_link::Message;
    use std::sync::mpsc::Receiver;

    fn entry(id: &str, agent: &str, hostname: &str) -> AgentEntry {
        AgentEntry {
            id: id.into(),
            agent: agent.into(),
            state: AgentState::Working,
            state_since: 7,
            cwd: "/remote/project".into(),
            pid: 42,
            args: vec![agent.into()],
            hostname: hostname.into(),
            started_at: 7,
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

    fn observer() -> (Observer, Receiver<Message>) {
        let (link, rx) = DaemonLink::test();
        let observer = Observer::new(
            Setup {
                agent_id: "own".into(),
                agent: None,
                cwd: PathBuf::from("/"),
                cols: 80,
                rows: 24,
                entry: entry("own", "ssh", "here"),
            },
            link,
        )
        .expect("shadow terminal");
        (observer, rx)
    }

    fn sent(rx: &Receiver<Message>) -> Vec<Message> {
        rx.try_iter().collect()
    }

    fn register(remote: AgentEntry) -> Signal {
        Signal::Remote(RemoteReport::Register(Box::new(remote)))
    }

    #[test]
    fn a_remote_register_takes_over_the_row() {
        let (mut observer, rx) = observer();
        observer.handle(Signal::Window {
            window: Some("w1".into()),
            workspace: Some("3".into()),
        });
        let _ = sent(&rx);

        observer.handle(register(entry("r1", "claude", "far")));

        match sent(&rx).as_slice() {
            [Message::Register(merged)] => {
                assert_eq!(merged.id, "own", "the row keeps its id");
                assert_eq!(merged.agent, "claude");
                assert_eq!(merged.hostname, "far");
                assert_eq!(merged.cwd, "/remote/project");
                assert_eq!(merged.window.as_deref(), Some("w1"), "grafted");
                assert_eq!(merged.workspace.as_deref(), Some("3"));
            }
            other => panic!("expected one register, got {} messages", other.len()),
        }
    }

    #[test]
    fn a_heartbeat_register_does_not_reset_the_state_clock() {
        let (mut observer, rx) = observer();
        observer.handle(register(entry("r1", "claude", "far")));
        std::thread::sleep(Duration::from_millis(3));
        observer.handle(register(entry("r1", "claude", "far")));

        let clocks: Vec<u64> = sent(&rx)
            .into_iter()
            .map(|message| match message {
                Message::Register(merged) => merged.state_since,
                _ => panic!("registers only"),
            })
            .collect();
        assert_eq!(clocks.len(), 2);
        assert_eq!(clocks[0], clocks[1], "same state, same clock");
    }

    #[test]
    fn remote_updates_are_re_addressed_and_re_stamped() {
        let (mut observer, rx) = observer();
        observer.handle(register(entry("r1", "claude", "far")));
        let _ = sent(&rx);

        let mut patch = AgentPatch::new("r1");
        patch.state = Some(AgentState::Blocked);
        patch.window = Some(Some("their-window".into()));
        observer.handle(Signal::Remote(RemoteReport::Update(Box::new(patch))));

        match sent(&rx).as_slice() {
            [Message::Update(patch)] => {
                assert_eq!(patch.id, "own");
                assert_eq!(patch.state, Some(AgentState::Blocked));
                assert!(patch.state_since.is_some(), "a state change is re-stamped");
                assert!(patch.window.is_none(), "placement stays local");
            }
            other => panic!("expected one update, got {} messages", other.len()),
        }

        // A patch that repeats the state carries no clock at all.
        let mut repeat = AgentPatch::new("r1");
        repeat.state = Some(AgentState::Blocked);
        repeat.state_since = Some(999);
        observer.handle(Signal::Remote(RemoteReport::Update(Box::new(repeat))));
        match sent(&rx).as_slice() {
            [Message::Update(patch)] => assert!(patch.state_since.is_none()),
            other => panic!("expected one update, got {} messages", other.len()),
        }
    }

    #[test]
    fn local_detection_stays_quiet_while_a_remote_agent_owns_the_row() {
        let (mut observer, rx) = observer();
        observer.handle(register(entry("r1", "claude", "far")));
        let _ = sent(&rx);

        observer.handle(Signal::Focus(false));
        observer.handle(Signal::Branch(Some("main".into())));

        assert!(sent(&rx).is_empty(), "own patches are suppressed");
        assert_eq!(
            observer.own_entry.branch.as_deref(),
            Some("main"),
            "but still remembered for the revert"
        );
    }

    #[test]
    fn window_moves_still_reach_the_daemon_while_remote() {
        let (mut observer, rx) = observer();
        observer.handle(register(entry("r1", "claude", "far")));
        let _ = sent(&rx);

        observer.handle(Signal::Window {
            window: Some("w2".into()),
            workspace: Some("5".into()),
        });

        match sent(&rx).as_slice() {
            [Message::Update(patch)] => {
                assert_eq!(patch.id, "own");
                assert_eq!(patch.window, Some(Some("w2".into())));
            }
            other => panic!("expected one update, got {} messages", other.len()),
        }
    }

    #[test]
    fn bye_reverts_to_the_wrappers_own_entry() {
        let (mut observer, rx) = observer();
        observer.handle(register(entry("r1", "claude", "far")));
        let _ = sent(&rx);

        observer.handle(Signal::Remote(RemoteReport::Bye));

        match sent(&rx).as_slice() {
            [Message::Register(own)] => {
                assert_eq!(own.id, "own");
                assert_eq!(own.agent, "ssh");
            }
            other => panic!("expected one register, got {} messages", other.len()),
        }
    }

    #[test]
    fn silence_reverts_the_row() {
        let (mut observer, rx) = observer();
        observer.handle(register(entry("r1", "claude", "far")));
        let _ = sent(&rx);

        observer.check_remote_liveness();
        assert!(sent(&rx).is_empty(), "a fresh whisper keeps the row");

        observer.remote.as_mut().expect("mirrored").last_heard = Instant::now()
            .checked_sub(WHISPER_TIMEOUT + Duration::from_secs(1))
            .expect("uptime");
        observer.check_remote_liveness();

        match sent(&rx).as_slice() {
            [Message::Register(own)] => assert_eq!(own.agent, "ssh"),
            other => panic!("expected one register, got {} messages", other.len()),
        }
    }
}
