//! `amon` — run a coding agent and hear back from it.
//!
//! The CLI is clap with external subcommands: any first token that is not a
//! amon subcommand names an agent to wrap, and everything after that token
//! belongs to the agent verbatim — `amon claude --help` gives the agent its
//! `--help`, not amon's (ADR-0006). Amon's own subcommands are strict:
//! unknown flags are errors, not noise.

use std::ffi::OsString;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::process::ExitCode;

use amon_protocol::{
    AgentEntry, AgentState, Hello, Method, ReportSession, ReportState, Request, Response, Role,
    StatusResult, PROTOCOL_VERSION,
};
use clap::{Parser, Subcommand};

mod doctor;
mod focus;
mod setup;
mod starter;

const VERSION: &str = env!("CARGO_PKG_VERSION");

const AFTER_HELP: &str = "\
Anything that is not a subcommand above names an agent to wrap:
  amon <agent> [args...]     everything after the agent's name is the agent's

The daemon starts on demand, so you rarely need `amon daemon`.

Detection, hook installation and manifests are derived from herdr
(https://github.com/ogulcancelik/herdr), Apache-2.0.";

#[derive(Parser)]
#[command(
    name = "amon",
    version,
    about = "Run coding agents and hear back from them",
    after_help = AFTER_HELP,
    arg_required_else_help = true
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// What every connected agent is doing
    Status {
        /// Machine-readable output
        #[arg(long)]
        json: bool,
    },
    /// Set up integrations — interactively, all detected, or one target
    Setup {
        /// One agent, e.g. `claude`; omit for the interactive screen
        target: Option<String>,
        /// Every detected agent plus the bar widget, without a screen
        #[arg(long, conflicts_with = "target")]
        all: bool,
        /// Do not alias the agent's own name to run it under amon
        /// (non-interactive forms only — the screen always aliases)
        #[arg(long)]
        no_alias: bool,
    },
    /// Remove integrations — everything after one confirmation, or one target
    Remove {
        /// One agent, e.g. `claude`; omit to remove everything
        target: Option<String>,
        /// Skip the confirmation (for scripts)
        #[arg(long, conflicts_with = "target")]
        all: bool,
    },
    /// Go to a workspace, landing on the agent that needs your attention
    /// rather than on whatever was focused there last
    Focus {
        /// Workspace number, e.g. `3`
        workspace: u32,
    },
    /// Integration, daemon, widget, and alias health in one report
    Doctor,
    /// Run the daemon in the foreground
    Daemon,
    /// Relay an installed hook's report to its wrapper (used by hook scripts)
    #[command(subcommand)]
    Hook(HookReport),
    /// Anything else wraps an agent; everything after its name is the agent's
    /// — verbatim, including bytes that are not UTF-8 (ADR-0006).
    #[command(external_subcommand)]
    Agent(Vec<OsString>),
}

#[derive(Subcommand)]
enum HookReport {
    /// Report which session the agent is in
    #[command(name = "report-agent-session")]
    Session {
        /// The `AMON_AGENT_ID` the hook was given
        agent_id: String,
        #[arg(long)]
        source: String,
        #[arg(long)]
        agent: String,
        #[arg(long)]
        seq: u64,
        #[arg(long)]
        agent_session_id: String,
        #[arg(long)]
        agent_session_path: Option<String>,
        #[arg(long)]
        session_start_source: Option<String>,
    },
    /// Report the agent's state
    #[command(name = "report-agent")]
    State {
        /// The `AMON_AGENT_ID` the hook was given
        agent_id: String,
        #[arg(long)]
        source: String,
        #[arg(long)]
        agent: String,
        #[arg(long, value_parser = parse_state)]
        state: AgentState,
        #[arg(long)]
        seq: u64,
        #[arg(long)]
        agent_session_id: Option<String>,
    },
}

fn parse_state(value: &str) -> Result<AgentState, String> {
    match value {
        "idle" => Ok(AgentState::Idle),
        "working" => Ok(AgentState::Working),
        "blocked" => Ok(AgentState::Blocked),
        "unknown" => Ok(AgentState::Unknown),
        other => Err(format!(
            "unknown state: {other} (expected idle, working, blocked, or unknown)"
        )),
    }
}

fn main() -> ExitCode {
    let cli = Cli::parse();

    let result = match cli.command {
        Command::Agent(argv) => return run_agent(&argv),
        Command::Status { json } => run_status(json),
        Command::Setup {
            target,
            all,
            no_alias,
        } => run_setup(target.as_deref(), all, no_alias),
        Command::Remove { target, all } => run_remove(target.as_deref(), all),
        Command::Focus { workspace } => focus::run(workspace),
        Command::Doctor => doctor::run(VERSION),
        Command::Daemon => run_daemon(),
        Command::Hook(report) => run_hook(report),
    };

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("amon: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run_agent(argv: &[OsString]) -> ExitCode {
    // A nudge, never a block: an outdated hook still works, it just knows less.
    if let Some(notice) = amon_integration::outdated_notice(&agent_name(&argv[0])) {
        eprintln!("{notice}");
    }

    match amon_wrapper::run(amon_wrapper::Launch {
        argv: argv.to_vec(),
        version: VERSION.to_string(),
    }) {
        Ok(amon_wrapper::AgentExit::Code(code)) => ExitCode::from(u8::try_from(code).unwrap_or(1)),
        // Die the way the agent died: a caller watching amon must see the
        // same signal death it would have seen without amon in the middle.
        Ok(amon_wrapper::AgentExit::Signal(signal)) => {
            unsafe {
                let mut mask: libc::sigset_t = std::mem::zeroed();
                libc::sigemptyset(&mut mask);
                libc::sigaddset(&mut mask, signal);
                libc::sigprocmask(libc::SIG_UNBLOCK, &mask, std::ptr::null_mut());
                libc::signal(signal, libc::SIG_DFL);
                libc::raise(signal);
            }
            // Only reachable if the signal did not kill us; fall back to the
            // shell convention for signal deaths.
            ExitCode::from(128u8.wrapping_add(signal as u8))
        }
        Err(error) => {
            eprintln!("amon: {error}");
            ExitCode::FAILURE
        }
    }
}

fn agent_name(program: &std::ffi::OsStr) -> String {
    std::path::Path::new(program)
        .file_name()
        .unwrap_or(program)
        .to_string_lossy()
        .into_owned()
}

fn run_daemon() -> Result<(), Box<dyn std::error::Error>> {
    amon_daemon::run(VERSION.to_string())?;
    Ok(())
}

fn run_status(json: bool) -> Result<(), Box<dyn std::error::Error>> {
    let agents = fetch_status()?;

    if json {
        println!("{}", serde_json::to_string_pretty(&agents)?);
        return Ok(());
    }
    if agents.is_empty() {
        println!("no agents running");
        return Ok(());
    }

    let width = agents
        .iter()
        .map(|agent| agent.agent.len())
        .max()
        .unwrap_or(5)
        .max(5);
    // Only earns a column when something is actually running on a compositor
    // amon understands; everywhere else the line stays as it was.
    let workspace_width = agents
        .iter()
        .filter_map(|agent| agent.workspace.as_deref())
        .map(str::len)
        .max();

    for agent in agents {
        let workspace = match workspace_width {
            Some(width) => format!(
                "{:<width$}  ",
                agent.workspace.as_deref().unwrap_or("-"),
                width = width
            ),
            None => String::new(),
        };
        println!(
            "{:<width$}  {:<8}  {:>5}  {workspace}{}",
            agent.agent,
            agent.state.as_str(),
            age(agent.state_since),
            agent.cwd,
            width = width
        );
    }
    Ok(())
}

/// How long the agent has been in its current state, in the shortest form that
/// is still unambiguous.
fn age(since_millis: u64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| since.as_millis() as u64)
        .unwrap_or(since_millis);
    let seconds = now.saturating_sub(since_millis) / 1000;
    match seconds {
        0..=59 => format!("{seconds}s"),
        60..=3599 => format!("{}m", seconds / 60),
        _ => format!("{}h", seconds / 3600),
    }
}

fn fetch_status() -> Result<Vec<AgentEntry>, Box<dyn std::error::Error>> {
    // Every command starts the daemon, status included: "no daemon" and "no
    // agents" are the same answer, and this keeps one code path.
    let mut client = Client::connect_or_start()?;
    let result = client.request(Method::Status)?;
    let status: StatusResult = serde_json::from_value(result.unwrap_or_default())?;
    Ok(status.agents)
}

fn run_setup(
    target: Option<&str>,
    all: bool,
    no_alias: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    // Before any form of setup, and only ever once: a commented file so the
    // settings can be found at all. Failing to write it is not a reason to
    // refuse to set anything up — it configures nothing by design.
    if let Ok(Some(path)) = starter::write_if_absent() {
        println!("wrote {} — every setting, commented out", path.display());
        println!();
    }
    if all {
        return setup::all(no_alias);
    }
    let Some(target) = target else {
        if !setup::is_tty() {
            return Err("the interactive screen needs a terminal; \
                 run `amon setup --all` or name a target (`amon setup claude`)"
                .into());
        }
        if no_alias {
            // The screen always aliases (ADR-0009, as amended); a silently
            // dropped flag would be worse than saying so.
            return Err("--no-alias only applies with a target or --all".into());
        }
        return setup::interactive();
    };
    setup_one(target, no_alias)
}

/// `amon setup <target>` — today's per-target form, and the escape hatch for
/// agents not detected on this machine.
///
/// A target is always an agent. The bar widget used to be nameable here too,
/// which put a desktop and an agent in the same slot for two operations that
/// are not alike: one installs a hook into a tool you run, the other installs
/// a subscriber into your desktop, and only one of them wants an alias. The
/// widget now comes from the screen or `--all`.
fn setup_one(target: &str, no_alias: bool) -> Result<(), Box<dyn std::error::Error>> {
    let agent = require_target(target)?;
    let mut messages = amon_integration::install(agent)?;
    if !no_alias {
        messages.push(String::new());
        messages.extend(amon_integration::alias::install(agent)?);
    }
    // Named targets get what the screen gives: the alias for shells, and the
    // shim for what Omarchy launches (ADR-0013). One of the two alone would
    // wrap an agent when you type its name and leave it bare on the keybinding.
    if amon_integration::desktop::cli_present() {
        amon_integration::shims::install(agent)?;
    }
    for message in messages {
        println!("{message}");
    }
    Ok(())
}

fn run_remove(target: Option<&str>, all: bool) -> Result<(), Box<dyn std::error::Error>> {
    let Some(target) = target else {
        if !all && !setup::is_tty() {
            return Err("removing everything needs a confirmation; \
                 run `amon remove --all` or name a target (`amon remove claude`)"
                .into());
        }
        return setup::remove_everything(all);
    };
    // No flag on the way out: an alias left behind for an agent amon no longer
    // hooks would keep taking over its name for nothing.
    //
    // Agents only, matching setup. The widget comes out with the screen or
    // `amon remove --all`.
    let agent = require_target(target)?;
    let mut messages = amon_integration::uninstall(agent)?;
    messages.extend(amon_integration::alias::uninstall(agent)?);
    // The third thing this integration owned. Left behind it would go on
    // taking over the agent's name for everything Omarchy launches.
    amon_integration::shims::uninstall(agent)?;
    // Aliases stranded in a symlinked bashrc survive the uninstall, which
    // refuses to write through links — name this agent's lines rather than
    // leaving them silently active (and only its lines: other agents' aliases
    // are still wanted).
    messages.extend(amon_integration::alias::stranded_for(agent));
    for message in messages {
        println!("{message}");
    }
    Ok(())
}

fn require_target(
    name: &str,
) -> Result<amon_integration::IntegrationTarget, Box<dyn std::error::Error>> {
    amon_integration::parse_target(name)
        .ok_or_else(|| format!("unknown target: {name}\n{}", known_targets()).into())
}

fn known_targets() -> String {
    let names: Vec<&str> = amon_integration::all_targets()
        .into_iter()
        .map(amon_integration::target_label)
        .collect();
    format!("known targets: {}", names.join(", "))
}

/// The relay for hooks that cannot speak the socket protocol directly and
/// shell out to the amon binary instead (herdr's `pane report-agent[-session]`,
/// renamed by the token map).
///
/// Argument errors fail loudly via clap — only humans and broken assets
/// produce them. Transport failures exit quietly: a hook runs inside someone's
/// coding session, so a missing socket or a dead wrapper must cost nothing.
fn run_hook(report: HookReport) -> Result<(), Box<dyn std::error::Error>> {
    let method = match report {
        HookReport::Session {
            agent_id,
            source,
            agent,
            seq,
            agent_session_id,
            agent_session_path,
            session_start_source,
        } => Method::AgentReportSession(ReportSession {
            agent_id,
            source,
            agent,
            seq,
            agent_session_id,
            agent_session_path,
            session_start_source,
        }),
        HookReport::State {
            agent_id,
            source,
            agent,
            state,
            seq,
            agent_session_id,
        } => Method::AgentReportState(ReportState {
            agent_id,
            source,
            agent,
            state,
            seq,
            agent_session_id,
        }),
    };

    let Some(socket) = std::env::var_os(amon_protocol::env::SOCKET_PATH) else {
        return Ok(());
    };
    let Ok(stream) = UnixStream::connect(socket) else {
        return Ok(());
    };
    let timeout = Some(std::time::Duration::from_secs(2));
    let _ = stream.set_read_timeout(timeout);
    let _ = stream.set_write_timeout(timeout);
    let request = Request::new("hook-1", method);
    let mut writer = &stream;
    if writer.write_all(request.to_line().as_bytes()).is_ok() {
        let _ = writer.flush();
        let mut line = String::new();
        let _ = BufReader::new(stream).read_line(&mut line);
    }
    Ok(())
}

/// A short-lived connection for one-shot commands.
pub(crate) struct Client {
    stream: UnixStream,
    reader: BufReader<UnixStream>,
    next_id: u64,
}

impl Client {
    fn connect_or_start() -> Result<Self, Box<dyn std::error::Error>> {
        // The connect-or-spawn primitive is shared with the wrapper's link.
        let stream = amon_protocol::connect_or_spawn_daemon()
            .ok_or("could not reach or start the amon daemon")?;
        stream.set_read_timeout(Some(std::time::Duration::from_secs(5)))?;
        Self::greet(stream)
    }

    /// Connects only to a daemon that is already listening, and gives it
    /// `timeout` to answer.
    ///
    /// For callers that must not pay to start one. `amon focus` runs from a
    /// keybinding, where a daemon launch would be a visible stall before the
    /// workspace changes, and where no daemon just means no agents to find —
    /// which is a plain workspace switch, not an error.
    pub(crate) fn connect_running(
        timeout: std::time::Duration,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let stream = UnixStream::connect(amon_protocol::paths::daemon_socket())?;
        stream.set_read_timeout(Some(timeout))?;
        // A daemon that accepts and then stops reading would otherwise block
        // the write instead of the read, and outlast the bound either way.
        stream.set_write_timeout(Some(timeout))?;
        Self::greet(stream)
    }

    fn greet(stream: UnixStream) -> Result<Self, Box<dyn std::error::Error>> {
        let reader = BufReader::new(stream.try_clone()?);
        let mut client = Self {
            stream,
            reader,
            next_id: 0,
        };
        client.request(Method::Hello(Hello {
            role: Role::Client,
            protocol: PROTOCOL_VERSION,
            version: VERSION.to_string(),
        }))?;
        Ok(client)
    }

    pub(crate) fn request(
        &mut self,
        method: Method,
    ) -> Result<Option<serde_json::Value>, Box<dyn std::error::Error>> {
        self.next_id += 1;
        let request = Request::new(format!("c{}", self.next_id), method);
        self.stream.write_all(request.to_line().as_bytes())?;
        self.stream.flush()?;

        let mut line = String::new();
        if self.reader.read_line(&mut line)? == 0 {
            return Err("the daemon closed the connection".into());
        }
        let response: Response = serde_json::from_str(&line)?;
        if let Some(error) = response.error {
            return Err(error.message.into());
        }
        Ok(response.result)
    }
}
