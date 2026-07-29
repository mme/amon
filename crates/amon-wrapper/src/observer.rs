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
    },
    /// An integration hook reported which session the agent is in.
    HookSession {
        source: String,
        agent: String,
        session_id: String,
        session_path: Option<String>,
    },
    AgentExited,
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
}

/// Starts the observer on its own thread.
///
/// The shadow terminal holds raw pointers into Ghostty and is not `Send`, so
/// it is built here rather than handed over — which also means a terminal that
/// cannot be created costs the agent nothing: observation stops, the agent
/// runs on.
pub fn spawn(
    agent_id: String,
    agent: Option<Agent>,
    cwd: PathBuf,
    cols: u16,
    rows: u16,
    link: DaemonLink,
    signals: Receiver<Signal>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(
        move || match Observer::new(agent_id, agent, cwd, cols, rows, link) {
            Ok(observer) => observer.run(signals),
            Err(error) => {
                eprintln!(
                    "amon: shadow terminal unavailable, state will not be reported: {error}"
                );
            }
        },
    )
}

impl Observer {
    fn new(
        agent_id: String,
        agent: Option<Agent>,
        cwd: PathBuf,
        cols: u16,
        rows: u16,
        link: DaemonLink,
    ) -> Result<Self, libghostty_vt::Error> {
        Ok(Self {
            shadow: ShadowTerminal::new(cols, rows)?,
            state: TerminalState::new(TerminalId::alloc(), cwd),
            agent,
            agent_id,
            link,
            last_reported: AgentState::Unknown,
            last_title: None,
            dirty: false,
            last_detection: Instant::now(),
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
            Signal::Resize { cols, rows } => {
                let _ = self.shadow.resize(cols, rows);
                self.dirty = true;
            }
            Signal::HookState {
                source,
                agent,
                state,
                seq,
            } => {
                // `set_hook_authority_at` is the real entry point; the
                // shorter `set_hook_authority` upstream is a test convenience.
                let change = self.state.set_hook_authority_at(
                    source,
                    agent,
                    to_detect_state(state),
                    None,
                    None,
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
            } => {
                // Session identity is metadata, not state: a hook telling us
                // which session it is in says nothing about whether the agent
                // is busy, so the arbitrated state is left alone.
                let mut patch = AgentPatch::new(&self.agent_id);
                patch.agent = Some(agent);
                patch.agent_session_id = Some(session_id);
                patch.agent_session_path = session_path;
                let _ = source;
                self.link.update(patch);
            }
            Signal::AgentExited => {
                self.dirty = true;
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
            patch.title = title;
            self.link.update(patch);
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

        let mut patch = AgentPatch::new(&self.agent_id);
        patch.state = Some(state);
        patch.state_since = Some(crate::now_millis());
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
