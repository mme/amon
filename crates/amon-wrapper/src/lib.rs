//! Running an agent behind amon.
//!
//! The contract is that the agent cannot tell. Its stdin, stdout, and terminal
//! size are exactly what they would be without amon; amon keeps a copy of the
//! output for the shadow terminal and answers nothing itself. If any part of
//! the observation machinery fails, the agent keeps running.

use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::{SystemTime, UNIX_EPOCH};

use amon_protocol::{env as protocol_env, AgentEntry, AgentState};

mod daemon_link;
mod hook_socket;
mod observer;
mod tty;

pub use observer::Signal;

/// How the agent was invoked.
pub struct Launch {
    /// The agent's own argv, program first, exactly as the user typed it.
    pub argv: Vec<String>,
    /// Version string this build reports to the daemon.
    pub version: String,
}

/// Runs the agent to completion and returns its exit code.
pub fn run(launch: Launch) -> std::io::Result<i32> {
    let (program, args) = launch
        .argv
        .split_first()
        .ok_or_else(|| std::io::Error::other("no agent command given"))?;

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

    let agent_label = program.rsplit('/').next().unwrap_or(program).to_string();
    let agent = amon_detect::parse_agent_label(&agent_label);
    link.register(AgentEntry {
        id: agent_id.clone(),
        agent: agent_label,
        state: AgentState::Unknown,
        state_since: now_millis(),
        cwd: cwd.to_string_lossy().into_owned(),
        pid: child.process_id().unwrap_or(0),
        args: launch.argv.clone(),
        hostname: hostname(),
        started_at: now_millis(),
        title: None,
        agent_session_id: None,
        agent_session_path: None,
    });

    let observer_thread = observer::spawn(agent_id, agent, cwd, cols, rows, link, inbox);

    let raw = tty::RawMode::enter()?;

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
    std::thread::spawn(move || {
        let mut stdin = std::io::stdin();
        let mut buffer = [0u8; 4096];
        while let Ok(read) = stdin.read(&mut buffer) {
            if read == 0 {
                return;
            }
            let Ok(mut writer) = stdin_writer.lock() else {
                return;
            };
            if writer.write_all(&buffer[..read]).is_err() {
                return;
            }
            let _ = writer.flush();
        }
    });

    let mut reader = pty
        .master
        .try_clone_reader()
        .map_err(std::io::Error::other)?;
    let mut stdout = std::io::stdout();
    let mut buffer = [0u8; 8192];
    loop {
        if resized.swap(false, std::sync::atomic::Ordering::Relaxed) {
            let (cols, rows) = tty::window_size();
            let _ = pty.master.resize(portable_pty::PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            });
            // The shadow must match the real terminal or detection reads a
            // differently-wrapped screen than the user sees.
            let _ = signals.send(Signal::Resize { cols, rows });
        }

        match reader.read(&mut buffer) {
            Ok(0) | Err(_) => break,
            Ok(read) => {
                // The user's terminal comes first, always. Observation is a
                // copy taken afterwards; it can neither delay nor alter this.
                if stdout.write_all(&buffer[..read]).is_err() {
                    break;
                }
                let _ = stdout.flush();
                let _ = signals.send(Signal::Output(buffer[..read].to_vec()));
            }
        }
    }

    let status = child.wait().map_err(std::io::Error::other)?;
    let _ = signals.send(Signal::AgentExited);
    drop(signals);
    let _ = observer_thread.join();
    drop(raw);
    drop(hook_socket);

    Ok(status.exit_code() as i32)
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
