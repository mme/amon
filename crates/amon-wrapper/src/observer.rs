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
use amon_protocol::{AgentPatch, AgentState};
use amon_term::{ShadowTerminal, TerminalId, TerminalState};

use crate::daemon_link::DaemonLink;

/// How often detection runs at most. Agents repaint far more often than this,
/// and a spinner frame does not need to be classified twice.
const DETECTION_INTERVAL: Duration = Duration::from_millis(150);

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
    /// Where the agent is working changed: it moved, or HEAD moved under it.
    /// Carries all of it at once, because a row showing a branch from one
    /// checkout beside a Project from another would be wrong about both.
    Location(crate::git::Location),
    AgentExited,
}

pub struct Observer {
    shadow: ShadowTerminal,
    state: TerminalState,
    agent: Option<Agent>,
    agent_id: String,
    link: DaemonLink,
    last_reported: AgentState,
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
    /// What the agent says it is doing, read off the same screen detection
    /// reads. Held across frames it cannot read, so a blinking marker does
    /// not make the row flicker.
    activity: amon_term::ActivityTracker,
    /// The activity last reported, so the daemon sees changes rather than a
    /// heartbeat — the same rule the state field follows.
    last_activity: Option<String>,
}

/// What the observer needs to know about the agent it is watching.
pub struct Setup {
    pub agent_id: String,
    pub agent: Option<Agent>,
    pub cwd: PathBuf,
    pub cols: u16,
    pub rows: u16,
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
            dirty: false,
            last_detection: Instant::now(),
            identity_seqs: std::collections::HashMap::new(),
            last_session_id: None,
            focus: crate::focus::Tracker::default(),
            activity: amon_term::ActivityTracker::new(),
            last_activity: None,
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
                    self.link.update(patch);
                }
            }
            Signal::Window { window, workspace } => {
                let mut patch = AgentPatch::new(&self.agent_id);
                patch.window = Some(window);
                patch.workspace = Some(workspace);
                self.link.update(patch);
            }
            Signal::Location(location) => {
                let mut patch = AgentPatch::new(&self.agent_id);
                patch.cwd = Some(location.cwd.to_string_lossy().into_owned());
                patch.project = Some(location.project);
                patch.subpath = Some(location.subpath);
                patch.branch = Some(location.branch);
                self.link.update(patch);
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
                        self.link.update(patch);
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
                    // A different session is a different piece of work, and
                    // what the last one was doing is not a fact about this
                    // one. Only a *replaced* session clears: the first report
                    // names the session the screen was already showing.
                    if !same_session && self.last_session_id.is_some() {
                        self.activity.clear();
                    }
                    self.last_session_id = Some(session_id.clone());
                    let mut patch = AgentPatch::new(&self.agent_id);
                    patch.agent = Some(agent.clone());
                    patch.agent_session_id = Some(session_id.clone());
                    patch.agent_session_path = session_path.clone();
                    self.link.update(patch);
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
        let title = title.as_deref().unwrap_or_default();
        let screen = self.shadow.detection_text();
        let detection = detect_agent_with_osc(self.agent, &screen, title, "");

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

        // Read from the same frame detection just read, so the row's state
        // and the row's account of itself can never describe different
        // screens. An agent amon could not identify has no carrier and would
        // read nothing anyway.
        if let Some(agent) = self.agent {
            self.activity.observe(
                agent,
                amon_detect::detect::manifest::DetectionInput {
                    screen: &screen,
                    osc_title: title,
                    osc_progress: "",
                },
            );
        }

        self.publish();
    }

    /// Reports the arbitrated state and the activity line, but only what
    /// actually changed — the daemon's subscribers should see transitions,
    /// not a heartbeat. The two move independently: an agent working its way
    /// through a long turn narrates several steps without changing state, and
    /// a state change can arrive on a frame whose activity is unchanged.
    fn publish(&mut self) {
        let state = from_detect_state(self.state.state);
        let activity = self.activity.current().map(str::to_string);
        let state_changed = state != self.last_reported;
        let activity_changed = activity != self.last_activity;
        if !state_changed && !activity_changed {
            return;
        }

        let mut patch = AgentPatch::new(&self.agent_id);
        if state_changed {
            self.last_reported = state;
            // Seen is per-state: whatever the user saw of the last state says
            // nothing about this one.
            self.focus.state_began();
            patch.state = Some(state);
            patch.state_since = Some(crate::now_millis());
            patch.seen = Some(self.focus.seen());
        }
        if activity_changed {
            self.last_activity = activity.clone();
            patch.activity = Some(activity);
        }
        self.link.update(patch);
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
