//! `amon` — run a coding agent and hear back from it.
//!
//! Argument handling is deliberately hand-rolled rather than derived: `amon
//! claude --help` must give the agent its `--help`, not amon's, and everything
//! after the agent's name belongs to the agent. A parser that knew about flags
//! would have to be told to stop knowing.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::process::ExitCode;

use amon_protocol::{
    paths, AgentEntry, Hello, Method, Request, Response, Role, StatusResult, PROTOCOL_VERSION,
};

const VERSION: &str = env!("CARGO_PKG_VERSION");

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(command) = args.first().map(String::as_str) else {
        print_help();
        return ExitCode::from(2);
    };

    let result = match command {
        "--help" | "-h" | "help" => {
            print_help();
            return ExitCode::SUCCESS;
        }
        "--version" | "-V" => {
            println!("amon {VERSION}");
            return ExitCode::SUCCESS;
        }
        "daemon" => run_daemon(),
        "status" => run_status(&args[1..]),
        "install" => run_install(&args[1..]),
        "uninstall" => run_uninstall(&args[1..]),
        "integrations" => run_integrations(),
        // Anything else names an agent to wrap, with the rest of argv as its.
        _ => return run_agent(&args),
    };

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("amon: {error}");
            ExitCode::FAILURE
        }
    }
}

fn print_help() {
    println!(
        "amon {VERSION} — run coding agents and hear back from them

usage:
  amon <agent> [args...]     wrap an agent; everything after it is the agent's
  amon status [--json]       what every connected agent is doing
  amon install <agent>       install this agent's integration hooks
  amon uninstall <agent>     remove them
  amon integrations          which integrations are installed, and how current
  amon daemon                run the daemon in the foreground

The daemon starts on demand, so you rarely need the last one.

Detection, hook installation and manifests are derived from herdr
(https://github.com/ogulcancelik/herdr), Apache-2.0."
    );
}

fn run_agent(argv: &[String]) -> ExitCode {
    // A nudge, never a block: an outdated hook still works, it just knows less.
    if let Some(notice) = amon_integration::outdated_notice(&agent_name(&argv[0])) {
        eprintln!("{notice}");
    }

    match amon_wrapper::run(amon_wrapper::Launch {
        argv: argv.to_vec(),
        version: VERSION.to_string(),
    }) {
        Ok(code) => ExitCode::from(u8::try_from(code).unwrap_or(1)),
        Err(error) => {
            eprintln!("amon: {error}");
            ExitCode::FAILURE
        }
    }
}

fn agent_name(program: &str) -> String {
    program.rsplit('/').next().unwrap_or(program).to_string()
}

fn run_daemon() -> Result<(), Box<dyn std::error::Error>> {
    amon_daemon::run(VERSION.to_string())?;
    Ok(())
}

fn run_status(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let json = args.iter().any(|arg| arg == "--json");
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
    for agent in agents {
        println!(
            "{:<width$}  {:<8}  {:>5}  {}",
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

fn run_install(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let target = require_target(args, "install")?;
    for message in amon_integration::install(target)? {
        println!("{message}");
    }
    Ok(())
}

fn run_uninstall(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let target = require_target(args, "uninstall")?;
    for message in amon_integration::uninstall(target)? {
        println!("{message}");
    }
    Ok(())
}

fn require_target(
    args: &[String],
    action: &str,
) -> Result<amon_integration::IntegrationTarget, Box<dyn std::error::Error>> {
    let Some(name) = args.first() else {
        return Err(format!("usage: amon {action} <agent>\n{}", known_agents()).into());
    };
    amon_integration::parse_target(name)
        .ok_or_else(|| format!("unknown agent: {name}\n{}", known_agents()).into())
}

fn known_agents() -> String {
    let names: Vec<&str> = amon_integration::all_targets()
        .into_iter()
        .map(amon_integration::target_label)
        .collect();
    format!("known agents: {}", names.join(", "))
}

fn run_integrations() -> Result<(), Box<dyn std::error::Error>> {
    for status in amon_integration::statuses() {
        let state = match status.state {
            amon_integration::InstallState::NotInstalled => "not installed".to_string(),
            amon_integration::InstallState::Current => {
                format!("current (v{})", status.expected_version)
            }
            amon_integration::InstallState::Outdated => format!(
                "outdated (v{} < v{})",
                status
                    .installed_version
                    .map(|version| version.to_string())
                    .unwrap_or_else(|| "?".into()),
                status.expected_version
            ),
        };
        println!(
            "{:<12} {:<24} {}",
            status.label,
            state,
            status.path.display()
        );
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
        if let Ok(client) = Self::connect() {
            return Ok(client);
        }
        start_daemon()?;
        for _ in 0..40 {
            std::thread::sleep(std::time::Duration::from_millis(25));
            if let Ok(client) = Self::connect() {
                return Ok(client);
            }
        }
        Err("could not reach or start the amon daemon".into())
    }

    fn connect() -> Result<Self, Box<dyn std::error::Error>> {
        let stream = UnixStream::connect(paths::daemon_socket())?;
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

fn start_daemon() -> Result<(), Box<dyn std::error::Error>> {
    let exe = std::env::current_exe()?;
    std::process::Command::new(exe)
        .arg("daemon")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()?;
    Ok(())
}
