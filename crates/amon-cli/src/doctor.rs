//! `amon doctor` — one command that answers "why isn't amon working?".
//!
//! Grew out of `amon integrations` (ADR-0010): the listing is still here, with
//! the environment around it — daemon, omarchy CLI, widget, aliases. Every
//! check is read-only; doctor never starts a daemon and never repairs anything,
//! it only says what it found and which command fixes it.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;

use amon_integration::{alias, desktop, DesktopTarget, InstallState};
use amon_protocol::{Hello, HelloResult, Method, Request, Response, Role, PROTOCOL_VERSION};

pub fn run(version: &str) -> Result<(), Box<dyn std::error::Error>> {
    println!("integrations:");
    for status in amon_integration::statuses() {
        let installed = status
            .installed_version
            .map(|version| version.to_string())
            .unwrap_or_else(|| "?".into());
        print_line(
            status.label,
            &describe_state(
                status.state,
                &installed,
                &status.expected_version.to_string(),
            ),
            &status.path.display().to_string(),
        );
    }
    for status in desktop::statuses() {
        let installed = status.installed_version.unwrap_or_else(|| "?".into());
        print_line(
            status.label,
            &describe_state(status.state, &installed, &status.expected_version),
            &status.path.display().to_string(),
        );
    }

    println!();
    println!("daemon:");
    let socket = amon_protocol::paths::daemon_socket();
    match daemon_version(version) {
        Some(running) => print_line(
            "amond",
            &format!("running (v{running})"),
            &socket.display().to_string(),
        ),
        None => print_line(
            "amond",
            "not running (starts on demand)",
            &socket.display().to_string(),
        ),
    }

    println!();
    println!("omarchy:");
    if desktop::cli_present() {
        print_line("cli", "found", "");
        match desktop::enabled_in_bar(DesktopTarget::Omarchy) {
            Some(true) => print_line("widget", "enabled in the bar", ""),
            Some(false) => print_line(
                "widget",
                "not in the bar (omarchy plugin enable sh.amon.workspaces)",
                "",
            ),
            None => print_line("widget", "unknown (shell not reachable)", ""),
        }
    } else {
        print_line("cli", "not found (no bar to integrate with)", "");
    }

    println!();
    println!("aliases:");
    let aliases = alias::installed();
    if aliases.is_empty() {
        let file = alias::shell_config()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "~/.bashrc".into());
        print_line("bashrc", &format!("none in {file}"), "");
    } else {
        for line in aliases {
            print_line("bashrc", &line, "");
        }
    }
    Ok(())
}

/// Asks a *running* daemon for its version. Deliberately not connect-or-spawn:
/// a diagnostic that starts the thing it is diagnosing reports only on itself.
fn daemon_version(version: &str) -> Option<String> {
    let stream = UnixStream::connect(amon_protocol::paths::daemon_socket()).ok()?;
    let timeout = Some(std::time::Duration::from_secs(2));
    let _ = stream.set_read_timeout(timeout);
    let _ = stream.set_write_timeout(timeout);
    let request = Request::new(
        "doctor-hello",
        Method::Hello(Hello {
            role: Role::Client,
            protocol: PROTOCOL_VERSION,
            version: version.to_string(),
        }),
    );
    let mut writer = &stream;
    writer.write_all(request.to_line().as_bytes()).ok()?;
    let mut line = String::new();
    BufReader::new(stream).read_line(&mut line).ok()?;
    let response: Response = serde_json::from_str(&line).ok()?;
    let result: HelloResult = serde_json::from_value(response.result?).ok()?;
    Some(result.version)
}

fn describe_state(state: InstallState, installed: &str, expected: &str) -> String {
    match state {
        InstallState::NotInstalled => "not installed".to_string(),
        InstallState::Current => format!("current (v{expected})"),
        InstallState::Outdated => format!("outdated (v{installed} < v{expected})"),
    }
}

fn print_line(label: &str, state: &str, path: &str) {
    if path.is_empty() {
        println!("  {label:<12} {state}");
    } else {
        println!("  {label:<12} {state:<24} {path}");
    }
}
