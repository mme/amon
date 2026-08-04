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
    /// Install an integration: an agent's hooks, or a desktop's widget
    Install {
        /// What to install for, e.g. `claude` or `omarchy`
        target: String,
        /// Do not alias the agent's own name to run it under amon
        #[arg(long)]
        no_alias: bool,
    },
    /// Remove an installed integration
    Uninstall {
        /// What to uninstall from
        target: String,
    },
    /// Which integrations are installed, and how current
    Integrations,
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
        Command::Install { target, no_alias } => run_install(&target, no_alias),
        Command::Uninstall { target } => run_uninstall(&target),
        Command::Integrations => run_integrations(),
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

fn run_install(target: &str, no_alias: bool) -> Result<(), Box<dyn std::error::Error>> {
    // A desktop target installs a subscriber rather than an agent hook; same
    // verb, different direction. It gets no alias: nothing is typed to start a
    // bar widget.
    let messages = match amon_integration::desktop::parse_target(target) {
        Some(desktop) => amon_integration::desktop::install(desktop)?,
        None => {
            let agent = require_target(target)?;
            let mut messages = amon_integration::install(agent)?;
            if !no_alias {
                messages.push(String::new());
                messages.extend(amon_integration::alias::install(agent)?);
            }
            messages
        }
    };
    for message in messages {
        println!("{message}");
    }
    Ok(())
}

fn run_uninstall(target: &str) -> Result<(), Box<dyn std::error::Error>> {
    // No flag on the way out: an alias left behind for an agent amon no longer
    // hooks would keep taking over its name for nothing.
    let messages = match amon_integration::desktop::parse_target(target) {
        Some(desktop) => amon_integration::desktop::uninstall(desktop)?,
        None => {
            let agent = require_target(target)?;
            let mut messages = amon_integration::uninstall(agent)?;
            messages.extend(amon_integration::alias::uninstall(agent)?);
            messages
        }
    };
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
    let mut names: Vec<&str> = amon_integration::all_targets()
        .into_iter()
        .map(amon_integration::target_label)
        .collect();
    names.extend(
        amon_integration::DesktopTarget::ALL
            .into_iter()
            .map(amon_integration::DesktopTarget::label),
    );
    format!("known targets: {}", names.join(", "))
}

fn run_integrations() -> Result<(), Box<dyn std::error::Error>> {
    for status in amon_integration::statuses() {
        let installed = status
            .installed_version
            .map(|version| version.to_string())
            .unwrap_or_else(|| "?".into());
        print_integration(
            status.label,
            &describe_state(
                status.state,
                &installed,
                &status.expected_version.to_string(),
            ),
            &status.path,
        );
    }
    // Desktop integrations report the same three states; only their version is
    // a string rather than a number.
    for status in amon_integration::desktop::statuses() {
        let installed = status.installed_version.unwrap_or_else(|| "?".into());
        print_integration(
            status.label,
            &describe_state(status.state, &installed, &status.expected_version),
            &status.path,
        );
    }
    Ok(())
}

fn describe_state(
    state: amon_integration::InstallState,
    installed: &str,
    expected: &str,
) -> String {
    match state {
        amon_integration::InstallState::NotInstalled => "not installed".to_string(),
        amon_integration::InstallState::Current => format!("current (v{expected})"),
        amon_integration::InstallState::Outdated => {
            format!("outdated (v{installed} < v{expected})")
        }
    }
}

fn print_integration(label: &str, state: &str, path: &std::path::Path) {
    println!("{:<12} {:<24} {}", label, state, path.display());
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
struct Client {
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

    fn request(
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
