//! Running an agent behind amon.
//!
//! The contract is that the agent cannot tell. Its stdin, stdout, and terminal
//! size are exactly what they would be without amon; amon keeps a copy of the
//! output for the shadow terminal and answers nothing itself. If any part of
//! the observation machinery fails, the agent keeps running.

use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::mpsc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use amon_protocol::{env as protocol_env, AgentEntry, AgentState};

mod branch;
mod daemon_link;
mod focus;
mod hook_socket;
mod hypr;
mod observer;
mod tty;

pub use observer::Signal;

/// How the agent was invoked.
pub struct Launch {
    /// The agent's own argv, program first, exactly as the user typed it.
    /// `OsString`, not `String`: execve fidelity includes bytes that are not
    /// UTF-8 (ADR-0006). Telemetry copies are lossy — JSON cannot carry them.
    pub argv: Vec<std::ffi::OsString>,
    /// Version string this build reports to the daemon.
    pub version: String,
}

/// How the agent ended. A signal death is reported as the signal, not folded
/// into an exit code: the caller reproduces it so that running an agent under
/// amon ends indistinguishably from running it bare.
pub enum AgentExit {
    Code(i32),
    Signal(i32),
}

/// Set by herdr in every pane process it manages. Its contract, not ours —
/// the exact name and value herdr documents for integrations.
const HERDR_ENV: &str = "HERDR_ENV";

/// Runs the agent to completion and returns how it ended.
pub fn run(launch: Launch) -> std::io::Result<AgentExit> {
    let (program, args) = launch
        .argv
        .split_first()
        .ok_or_else(|| std::io::Error::other("no agent command given"))?;

    // Inside a herdr pane, amon steps aside entirely. The user's alias still
    // expands `claude` to `amon claude` there, but wrapping would be the
    // worst of both worlds: herdr sees an unrecognized `amon` process and
    // shows no agent, while the wrapper's window walk climbs pane shell →
    // herdr server → init and registers a row with nowhere to jump to.
    // Run bare instead: herdr detects the agent natively, and the daemon's
    // herdr module carries it onto the bar with the client's window — one
    // detection authority per context (docs/research/herdr-live-integration.md).
    //
    // exec, not spawn: the agent takes this process over, so signals, exit
    // codes, and the terminal behave exactly as if amon was never typed. If
    // the PATH lookup lands on an amon shim, the shim strips its own
    // directory before re-invoking amon, so the bounce terminates at the
    // real agent.
    if std::env::var_os(HERDR_ENV).is_some_and(|value| value == "1") {
        use std::os::unix::process::CommandExt;
        let mut bare = std::process::Command::new(program);
        bare.args(args);
        // An outer wrapper's routing must not follow the agent in here.
        // herdr started from inside a wrapped agent hands `AMON_*` down to
        // its server and every pane, and an agent that believes it is
        // wrapped reports its state to a wrapper that is watching something
        // else entirely — one agent's hooks driving another agent's row.
        bare.env_remove(protocol_env::AMON_ENV);
        bare.env_remove(protocol_env::AGENT_ID);
        bare.env_remove(protocol_env::SOCKET_PATH);
        // Only reached when exec itself failed — agent missing or not
        // executable, the same failure spawning it under a PTY would hit.
        return Err(bare.exec());
    }

    let agent_id = new_agent_id();
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let (cols, rows) = tty::window_size();
    let resized = tty::watch_resizes();

    let (signals, inbox) = mpsc::channel();
    // Bound to this function so the socket file is unlinked however we leave.
    let hook_socket = hook_socket::listen(&agent_id, signals.clone());
    let link = daemon_link::DaemonLink::spawn(launch.version);

    let pty = portable_pty::native_pty_system()
        .openpty(portable_pty::PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(std::io::Error::other)?;

    let mut command = portable_pty::CommandBuilder::new(program);
    command.args(args);
    command.cwd(&cwd);
    // What lets the agent's hooks find their way back to this wrapper.
    command.env(protocol_env::AMON_ENV, protocol_env::AMON_ENV_VALUE);
    command.env(protocol_env::AGENT_ID, &agent_id);
    if let Some(socket) = hook_socket.as_ref() {
        command.env(protocol_env::SOCKET_PATH, socket.path());
    }

    let mut child = pty
        .slave
        .spawn_command(command)
        .map_err(std::io::Error::other)?;
    // Dropping our end of the slave now means the PTY reports EOF when the
    // agent exits, instead of hanging because amon still holds it open.
    drop(pty.slave);

    // Signals sent to amon mean "stop the agent" — forward them so the agent
    // sees the real signal and the wrapper still unwinds normally.
    if let Some(pid) = child.process_id() {
        tty::forward::arm(pid);
    }

    let agent_label = agent_label(program);
    let agent = amon_detect::parse_agent_label(&agent_label);
    link.register(AgentEntry {
        id: agent_id.clone(),
        agent: agent_label,
        state: AgentState::Unknown,
        state_since: now_millis(),
        cwd: cwd.to_string_lossy().into_owned(),
        pid: child.process_id().unwrap_or(0),
        args: launch
            .argv
            .iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect(),
        hostname: hostname(),
        started_at: now_millis(),
        title: None,
        agent_session_id: None,
        agent_session_path: None,
        window: None,
        workspace: None,
        // Read before the agent is registered, so a row never appears without
        // its branch and then acquires one a second later.
        branch: branch::read(&cwd),
        focused: None,
        seen: None,
        herdr: None,
    });

    // Kept before `Setup` takes ownership of `cwd`; the watcher outlives this
    // scope and needs its own copy.
    let watched_directory = cwd.clone();
    let focus_shared = focus::Shared::default();
    let observer_thread = observer::spawn(
        observer::Setup {
            agent_id,
            agent,
            cwd,
            cols,
            rows,
        },
        link,
        inbox,
    );

    // Outside the terminal check below, deliberately. A window only exists for
    // an agent running in a terminal, so `hypr` is rightly guarded — but a
    // branch is a fact about a directory. `amon claude > log` has a cwd and a
    // branch like any other agent, and its row should say so.
    branch::watch(watched_directory, signals.clone());

    let raw = tty::RawMode::enter()?;
    // Only a real terminal has focus to report, and only a real terminal may
    // be written to: stdout is where the mode is enabled, and with
    // `amon agent > log` that is a file, not a terminal.
    let on_a_terminal = raw.is_some() && tty::stdout_is_terminal();
    // Everything amon writes to the terminal goes through here, so its own
    // bytes can never land inside a sequence the agent is in the middle of.
    let screen = std::sync::Arc::new(std::sync::Mutex::new(Screen::new(
        on_a_terminal,
        focus_shared.clone(),
    )));
    if on_a_terminal {
        if let Ok(mut screen) = screen.lock() {
            screen.enable_focus_reports();
        }
        // The user typed the command into this view, so they are looking at it
        // now. Any real report from the terminal overrules this immediately;
        // without it, an agent nobody ever switches away from would stay at
        // "focus unknown" for its whole life.
        let _ = signals.send(Signal::Focus(true));
        hypr::spawn(signals.clone());
    }

    // stdin -> agent. Detached: it blocks on the user's keyboard, and there is
    // nothing to wait for once the agent is gone.
    //
    // The writer is shared rather than moved because dropping it makes
    // portable-pty synthesize a newline and EOF into the PTY. Running out of
    // input is not the user pressing ^D: the echoed newline would appear in
    // the agent's output, and the ^D could make the agent quit. A terminal
    // whose input ends simply stops delivering input, so amon does the same
    // and keeps the writer alive until the agent is gone.
    let pty_writer = std::sync::Arc::new(std::sync::Mutex::new(
        pty.master.take_writer().map_err(std::io::Error::other)?,
    ));
    let stdin_writer = pty_writer.clone();
    let stdin_focus = focus_shared.clone();
    let stdin_signals = signals.clone();
    std::thread::spawn(move || {
        let mut stdin = std::io::stdin();
        let mut buffer = [0u8; 4096];
        while let Ok(read) = stdin.read(&mut buffer) {
            if read == 0 {
                return;
            }
            let chunk = &buffer[..read];

            // Focus reports are amon's unless the agent asked for them itself,
            // in which case they pass through like anything else it enabled
            // (ADR-0007).
            let mut forward = chunk;
            let scanned;
            if on_a_terminal {
                let scan = focus::scan(chunk);
                for gained in scan.events {
                    let _ = stdin_signals.send(Signal::Focus(gained));
                }
                if !stdin_focus.agent_wants.load(Ordering::Relaxed) {
                    scanned = scan.stripped;
                    if let Some(stripped) = scanned.as_deref() {
                        forward = stripped;
                    }
                }
            }
            if forward.is_empty() {
                continue;
            }

            let Ok(mut writer) = stdin_writer.lock() else {
                return;
            };
            if writer.write_all(forward).is_err() {
                return;
            }
            let _ = writer.flush();
        }
    });

    let mut reader = pty
        .master
        .try_clone_reader()
        .map_err(std::io::Error::other)?;

    // Resizes are applied from their own thread because the reader below sits
    // in a blocking read (restarted across signals): checking a flag there
    // would leave a silent agent — and the user's screen — at the old size
    // until the agent's next output byte. Polling the flag here keeps the PTY
    // and the shadow in step with the real terminal within ~50ms regardless.
    // The shadow must match the real terminal or detection reads a
    // differently-wrapped screen than the user sees. The thread dies with the
    // process; the wrapper's lifetime is the process's.
    // It also carries amon's only writes to the terminal while the agent runs:
    // it already wakes on a timer, and `Screen` decides when writing is safe.
    let master = pty.master;
    let resize_signals = signals.clone();
    // Weak, so that the wrapper's own `Screen` is what decides when the
    // terminal stops being written to: once it is dropped, this thread can no
    // longer reach a terminal amon has already restored.
    let injector_screen = std::sync::Arc::downgrade(&screen);
    std::thread::spawn(move || loop {
        std::thread::sleep(Duration::from_millis(50));

        // The agent turned focus reporting off — with a mode reset, or by
        // disabling it outright — which turned amon's off with it.
        let Some(screen) = injector_screen.upgrade() else {
            return;
        };
        if let Ok(mut screen) = screen.lock() {
            screen.reassert_focus_reports();
        }

        if resized.swap(false, std::sync::atomic::Ordering::Relaxed) {
            let (cols, rows) = tty::window_size();
            // The shadow is told before the PTY: the real terminal already
            // has the new size, and anything the agent redraws in response to
            // the ioctl below must not reach the observer ahead of the resize
            // — output signals and this one land in the same ordered channel.
            if resize_signals.send(Signal::Resize { cols, rows }).is_err() {
                return;
            }
            let _ = master.resize(portable_pty::PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            });
        }
    });

    let mut buffer = [0u8; 8192];
    loop {
        match reader.read(&mut buffer) {
            Ok(0) | Err(_) => break,
            Ok(read) => {
                let Ok(mut screen) = screen.lock() else {
                    break;
                };
                // The user's terminal comes first, always. Observation is a
                // copy taken afterwards; it can neither delay nor alter this.
                if screen.agent_output(&buffer[..read]).is_err() {
                    break;
                }
                drop(screen);
                let _ = signals.send(Signal::Output(buffer[..read].to_vec()));
            }
        }
    }

    let exit = wait_for_agent(&mut child);
    tty::forward::disarm();

    // Leave the terminal as the agent left it, and shut the door behind us so
    // the injector cannot reach it afterwards.
    if let Ok(mut screen) = screen.lock() {
        screen.finish();
    }
    let _ = signals.send(Signal::AgentExited);
    drop(signals);
    let _ = observer_thread.join();
    drop(raw);
    drop(hook_socket);

    Ok(exit)
}

/// The user's terminal, and everything amon is allowed to do to it.
///
/// One owner for all writes, because amon has two reasons to write — passing
/// the agent's output through, and keeping focus reporting on — and the second
/// must never land inside a sequence belonging to the first. The mode scanner
/// lives here rather than in the observer for the same reason it decides the
/// agent's input: the observer is best-effort, and this is not.
struct Screen {
    stdout: std::io::Stdout,
    scanner: focus::ModeScanner,
    focus: focus::Shared,
    /// Whether focus reporting is amon's business at all: piped output gets
    /// none of it, and neither does a terminal amon has already restored.
    tracking: bool,
}

impl Screen {
    fn new(on_a_terminal: bool, focus: focus::Shared) -> Self {
        Self {
            stdout: std::io::stdout(),
            scanner: focus::ModeScanner::default(),
            focus,
            tracking: on_a_terminal,
        }
    }

    fn enable_focus_reports(&mut self) {
        let _ = self.stdout.write_all(focus::ENABLE);
        let _ = self.stdout.flush();
    }

    /// Passes the agent's output through, following what it does to the modes
    /// on the way.
    ///
    /// The scan happens first so that the flags governing the agent's input
    /// are already correct by the time the terminal could act on the bytes —
    /// it costs a pass over the chunk, never a syscall.
    fn agent_output(&mut self, bytes: &[u8]) -> std::io::Result<()> {
        if self.tracking {
            let change = self.scanner.feed(bytes);
            self.focus
                .agent_wants
                .store(change.agent_wants, Ordering::Relaxed);
            // What matters is where the mode ended up: an agent that turned it
            // off and back on within one read left the terminal reporting, so
            // there is nothing to put back.
            if change.agent_wants {
                self.focus.reassert.store(false, Ordering::Relaxed);
            } else if change.disabled {
                self.focus.reassert.store(true, Ordering::Relaxed);
            }
        }
        self.stdout.write_all(bytes)?;
        self.stdout.flush()
    }

    /// Puts focus reporting back if the agent turned it off — but only between
    /// the agent's own sequences, which is the only moment amon's bytes cannot
    /// change the meaning of the agent's.
    fn reassert_focus_reports(&mut self) {
        if !self.tracking
            || !self.focus.reassert.load(Ordering::Relaxed)
            || !self.scanner.at_boundary()
        {
            return;
        }
        self.focus.reassert.store(false, Ordering::Relaxed);
        let _ = self.stdout.write_all(focus::ENABLE);
        let _ = self.stdout.flush();
    }

    /// Restores the terminal and stops amon writing to it.
    ///
    /// If the agent wanted focus reporting, it is the agent's to leave on —
    /// that is what would have happened without amon. Otherwise the mode was
    /// amon's alone and goes away with it.
    fn finish(&mut self) {
        if !self.tracking {
            return;
        }
        self.tracking = false;
        if !self.focus.agent_wants.load(Ordering::Relaxed) {
            let _ = self.stdout.write_all(focus::DISABLE);
            let _ = self.stdout.flush();
        }
    }
}

impl Drop for Screen {
    /// The terminal is restored however the wrapper leaves — including the
    /// early returns between enabling the mode and the agent's first byte, and
    /// a panic on the way. `finish` has already run in the ordinary case.
    fn drop(&mut self) {
        self.finish();
    }
}

/// Reaps the agent and reports how it ended.
///
/// This calls `waitpid` directly rather than portable-pty's `wait`, which
/// stringifies the signal (via `strsignal`) and reports exit code 1 for any
/// signal death — losing exactly the information the caller needs to die the
/// same way.
fn wait_for_agent(child: &mut Box<dyn portable_pty::Child + Send + Sync>) -> AgentExit {
    if let Some(pid) = child.process_id() {
        let mut status: libc::c_int = 0;
        loop {
            let reaped = unsafe { libc::waitpid(pid as libc::pid_t, &mut status, 0) };
            if reaped == pid as libc::pid_t {
                if libc::WIFSIGNALED(status) {
                    return AgentExit::Signal(libc::WTERMSIG(status));
                }
                if libc::WIFEXITED(status) {
                    return AgentExit::Code(libc::WEXITSTATUS(status));
                }
                // Neither exited nor signalled (stopped?): keep waiting.
                continue;
            }
            if std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            break;
        }
    }
    // No pid, or someone else reaped it: portable-pty's summary is all there is.
    match child.wait() {
        Ok(status) => AgentExit::Code(status.exit_code() as i32),
        Err(_) => AgentExit::Code(1),
    }
}

/// The agent's display label: the program's file name, lossily decoded. Only
/// the exec path needs the exact bytes; names are for humans and manifests.
fn agent_label(program: &std::ffi::OsStr) -> String {
    std::path::Path::new(program)
        .file_name()
        .unwrap_or(program)
        .to_string_lossy()
        .into_owned()
}

/// Identifies this wrapper's agent everywhere: the registry entry, the hook
/// socket path, and `AMON_AGENT_ID`. Process id plus launch time is unique per
/// machine without pulling in a uuid dependency.
fn new_agent_id() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|since| since.as_nanos())
        .unwrap_or(0);
    format!("{:x}-{:x}", std::process::id(), nanos)
}

pub(crate) fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|since| since.as_millis() as u64)
        .unwrap_or(0)
}

fn hostname() -> String {
    let mut buffer = [0 as libc::c_char; 256];
    if unsafe { libc::gethostname(buffer.as_mut_ptr(), buffer.len()) } != 0 {
        return String::new();
    }
    let bytes: Vec<u8> = buffer
        .iter()
        .take_while(|byte| **byte != 0)
        .map(|byte| *byte as u8)
        .collect();
    String::from_utf8_lossy(&bytes).into_owned()
}
