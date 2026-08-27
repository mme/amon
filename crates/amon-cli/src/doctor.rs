//! `amon doctor` — one command that answers "why isn't amon working?".
//!
//! Grew out of `amon integrations` (ADR-0010): the listing is still here, with
//! the environment around it — daemon, omarchy CLI, widget, aliases. Every
//! check is read-only; doctor never starts a daemon and never repairs anything,
//! it only says what it found and which command fixes it.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;

use amon_integration::{alias, desktop, ducking, shims, udev, DesktopTarget, InstallState};
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
            &where_it_is(status.state, &status.path),
        );
    }
    for status in desktop::statuses() {
        let installed = status.installed_version.unwrap_or_else(|| "?".into());
        print_line(
            status.label,
            &describe_state(status.state, &installed, &status.expected_version),
            &where_it_is(status.state, &status.path),
        );
    }

    println!();
    println!("daemon:");
    let socket = amon_protocol::paths::daemon_socket();
    match probe_daemon(version) {
        DaemonProbe::Running(running) => {
            print_line(
                "amond",
                &format!("running (v{running})"),
                &socket.display().to_string(),
            );
            // What the daemon is actually running on, from the daemon
            // itself: a file that failed to parse keeps the last good
            // configuration (ADR-0012), and this is the report the docs
            // promise. Only the error's first line — TOML errors span
            // several, and the row is a status, not a stack trace.
            match probe_config() {
                Some(Some(error)) => print_line(
                    "config",
                    &format!(
                        "not applied ({}) — keeping the previous settings",
                        error.lines().next().unwrap_or("unreadable")
                    ),
                    "",
                ),
                Some(None) => print_line("config", "ok", ""),
                None => {}
            }
        }
        // A socket that accepts and then will not talk is a wedged daemon,
        // which is the opposite of "starts on demand" — it *blocks* on-demand
        // starts, since the socket looks taken.
        DaemonProbe::Unresponsive => print_line(
            "amond",
            "unresponsive (accepts connections, hello failed — kill it)",
            &socket.display().to_string(),
        ),
        DaemonProbe::NotRunning => print_line(
            "amond",
            "not running (starts on demand)",
            &socket.display().to_string(),
        ),
    }

    println!();
    println!("omarchy:");
    if desktop::cli_present() {
        print_line("cli", "found", "");
        let state = desktop::status(DesktopTarget::Omarchy).state;
        match desktop::enabled_in_bar(DesktopTarget::Omarchy) {
            Some(true) => print_line("widget", "enabled in the bar", ""),
            // The fix depends on what is missing: only a *current* copy is
            // worth enabling as-is. An absent or stale one wants `amon setup`
            // first — recommending enable would either fail (never installed)
            // or activate the outdated copy.
            Some(false) if state == InstallState::Current => print_line(
                "widget",
                "not in the bar (omarchy plugin enable sh.amon.workspaces)",
                "",
            ),
            Some(false) => print_line(
                "widget",
                "not in the bar (amon setup installs it fresh and enables it)",
                "",
            ),
            None => print_line("widget", "unknown (shell not reachable)", ""),
        }
    } else {
        print_line("cli", "not found (no bar to integrate with)", "");
    }

    println!();
    println!("audio:");
    if ducking::available() {
        match ducking::status() {
            InstallState::Current => {
                print_line("ducking", "installed (music dips under notifications)", "")
            }
            InstallState::Outdated => print_line(
                "ducking",
                "installed but stale (amon setup --duck refreshes it)",
                "",
            ),
            InstallState::NotInstalled => {
                print_line("ducking", "not installed (amon setup --duck)", "")
            }
        }
    } else {
        print_line("ducking", "no wireplumber on PATH — nothing to duck", "");
    }

    println!();
    println!("devices:");
    match amon_daemon::devices::discover() {
        Some(found) => {
            if udev::accessible(&found.node) {
                print_line(
                    "micro2",
                    "connected (the daemon lights it)",
                    &found.node.display().to_string(),
                );
            } else {
                print_line(
                    "micro2",
                    "connected, not accessible (amon setup adds the udev rule)",
                    &found.node.display().to_string(),
                );
            }
            if !udev::accessible(std::path::Path::new("/dev/uinput")) {
                print_line(
                    "input",
                    "no /dev/uinput access — key and scroll actions stay inert (amon setup)",
                    "",
                );
            }
        }
        None => print_line("micro2", "not connected", ""),
    }

    println!();
    println!("aliases:");
    // What bash will actually read, symlinked rc included — a diagnostic
    // reporting "none" while aliases are active would point the wrong way.
    let aliases = alias::present();
    if aliases.is_empty() {
        let file = alias::shell_config()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "~/.bashrc".into());
        print_line("bashrc", &format!("none in {file}"), "");
    } else {
        for line in aliases {
            print_line("bashrc", &line, "");
        }
        if let Some(note) = alias::stranded() {
            print_line("bashrc", &note, "");
        }
    }

    // Reported beside the aliases because they are the same decision for a
    // different context: an agent aliased but not shimmed is wrapped when you
    // type its name and bare when Omarchy launches it, which is exactly the
    // split shims exist to close (ADR-0013). Only where Omarchy is, because
    // only there does anything read them.
    if desktop::cli_present() {
        let shimmed = shims::installed();
        if shimmed.is_empty() {
            print_line(
                "session",
                "no shims (Omarchy launches agents unwrapped)",
                "",
            );
        } else {
            // One line per file, with where it is — the same shape the
            // integrations use, because a shim is the same kind of fact: a
            // thing amon put somewhere that you may want to go and look at.
            let directory = shims::dir().unwrap_or_default();
            let stranded = shims::stranded();
            for command in shimmed {
                let note = if stranded.contains(&command) {
                    format!("{command} (no integration left)")
                } else {
                    command.clone()
                };
                print_line(
                    "session",
                    &note,
                    &directory.join(&command).display().to_string(),
                );
            }
        }
    }
    Ok(())
}

enum DaemonProbe {
    Running(String),
    /// The socket accepted, then the handshake failed, timed out, or made no
    /// sense — a daemon that exists but cannot be talked to.
    Unresponsive,
    NotRunning,
}

/// Asks the running daemon for its configuration state: `Some(Some(error))`
/// when the file on disk is not what the daemon runs on, `Some(None)` when
/// all is well, `None` when the daemon could not answer (its own row already
/// says so) — or is too old to know the field, which deserializes as absent.
fn probe_config() -> Option<Option<String>> {
    let stream = UnixStream::connect(amon_protocol::paths::daemon_socket()).ok()?;
    let timeout = Some(std::time::Duration::from_secs(2));
    let _ = stream.set_read_timeout(timeout);
    let _ = stream.set_write_timeout(timeout);
    let request = Request::new("doctor-config", Method::Config);
    let mut writer = &stream;
    writer.write_all(request.to_line().as_bytes()).ok()?;
    let mut line = String::new();
    BufReader::new(&stream).read_line(&mut line).ok()?;
    let response: Response = serde_json::from_str(&line).ok()?;
    let result: amon_protocol::ConfigResult = serde_json::from_value(response.result?).ok()?;
    Some(result.error)
}

/// Asks a *running* daemon for its version. Deliberately not connect-or-spawn:
/// a diagnostic that starts the thing it is diagnosing reports only on itself.
fn probe_daemon(version: &str) -> DaemonProbe {
    let Ok(stream) = UnixStream::connect(amon_protocol::paths::daemon_socket()) else {
        return DaemonProbe::NotRunning;
    };
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
    let handshake = || -> Option<String> {
        let mut writer = &stream;
        writer.write_all(request.to_line().as_bytes()).ok()?;
        let mut line = String::new();
        BufReader::new(&stream).read_line(&mut line).ok()?;
        let response: Response = serde_json::from_str(&line).ok()?;
        let result: HelloResult = serde_json::from_value(response.result?).ok()?;
        Some(result.version)
    };
    match handshake() {
        Some(running) => DaemonProbe::Running(running),
        None => DaemonProbe::Unresponsive,
    }
}

/// The path column, which is only ever a fact about something that is there.
///
/// A not-installed row used to print where the hook *would* go, which reads as
/// a list of files — every one of them absent. There is no "where" for a thing
/// that does not exist, so the column is left empty and the state says the rest.
fn where_it_is(state: InstallState, path: &std::path::Path) -> String {
    match state {
        InstallState::NotInstalled => String::new(),
        _ => path.display().to_string(),
    }
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
