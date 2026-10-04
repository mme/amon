//! Conformance between the installed hook scripts and the protocol.
//!
//! The hooks are a few lines of shell that hand the agent's JSON to
//! `amon hook input` (issue #77). These tests install a hook the way
//! `amon setup` does, run it the way the agent would - with this build's amon
//! as `AMON_BIN_PATH`, as the wrapper sets it - and parse what comes out of
//! the socket with the very types the daemon and wrapper use, which is the
//! same code path that would reject the frame in production.
//!
//! Each test points the integrations at a scratch config through the
//! environment, so each needs a process of its own: run with nextest.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use amon_integration::IntegrationTarget;
use amon_protocol::{ActivityKind, Method, Request};

const AMON: &str = env!("CARGO_BIN_EXE_amon");

/// Somewhere short enough for a unix socket path, unique per test process.
fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("amh{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("scratch dir");
    dir
}

fn set_env(key: &str, value: impl AsRef<std::ffi::OsStr>) {
    // SAFETY: nextest runs each test in its own process, before any thread.
    unsafe { std::env::set_var(key, value) };
}

/// Runs a hook as the agent would inside amon, and returns what it printed
/// and every frame it sent to the socket.
fn run_hook(hook: &Path, args: &[&str], stdin_json: &str, socket: &Path) -> (Output, Vec<String>) {
    let listener = UnixListener::bind(socket).expect("bind hook socket");
    listener.set_nonblocking(true).expect("nonblocking accept");

    let collector = std::thread::spawn(move || {
        // A hook that rightly reports nothing never connects; a bounded
        // accept keeps that from hanging the suite.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let mut frames = Vec::new();
        while std::time::Instant::now() < deadline {
            match listener.accept() {
                Ok((stream, _)) => {
                    let _ = stream.set_nonblocking(false);
                    let mut reader = BufReader::new(&stream);
                    let mut line = String::new();
                    if reader.read_line(&mut line).is_ok() && !line.trim().is_empty() {
                        frames.push(line.trim().to_owned());
                        // Answer as the wrapper does, so the hook returns.
                        let _ = (&stream).write_all(b"{\"id\":\"hook-1\",\"result\":{}}\n");
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(std::time::Duration::from_millis(25));
                }
                Err(_) => break,
            }
            if !frames.is_empty() {
                // One report per invocation; a short grace for a stray second.
                std::thread::sleep(std::time::Duration::from_millis(200));
                if let Ok((stream, _)) = listener.accept() {
                    let mut line = String::new();
                    let _ = stream.set_nonblocking(false);
                    let _ = BufReader::new(&stream).read_line(&mut line);
                    frames.push(line.trim().to_owned());
                }
                break;
            }
        }
        frames
    });

    let mut child = Command::new("sh")
        .arg(hook)
        .args(args)
        .env(amon_protocol::env::AMON_ENV, "1")
        .env(amon_protocol::env::SOCKET_PATH, socket)
        .env(amon_protocol::env::AGENT_ID, "agent-under-test")
        .env(amon_protocol::env::BIN_PATH, AMON)
        // The developer's own environment must not leak into the hook under
        // test: with an ambient CODEX_THREAD_ID, the codex hook rightly
        // suppresses its report.
        .env_remove("CODEX_THREAD_ID")
        .env_remove("GROK_SESSION_ID")
        .env_remove("CURSOR_VERSION")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("hook runs");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(stdin_json.as_bytes())
        .expect("write hook input");
    let output = child.wait_with_output().expect("hook exits");
    assert!(
        output.status.success(),
        "a hook must never fail its agent: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.stderr.is_empty(),
        "nor print into its session: {output:?}"
    );
    (output, collector.join().expect("collector"))
}

/// What an installed agent has before its hook goes in: the hook's folder,
/// and the config folder antigravity's installer checks for.
fn agent_dirs(home: &Path, hook: &Path) {
    std::fs::create_dir_all(hook.parent().expect("hook folder")).expect("hook folder");
    std::fs::create_dir_all(home.join(".gemini/config")).expect("antigravity config");
}

fn only_frame(frames: &[String]) -> Method {
    assert_eq!(
        frames.len(),
        1,
        "one report per hook invocation: {frames:?}"
    );
    Request::parse(&frames[0])
        .expect("the daemon can parse this frame")
        .method
}

#[test]
fn the_claude_hook_reports_a_session_the_daemon_can_parse() {
    let config = scratch("claude");
    set_env("CLAUDE_CONFIG_DIR", &config);
    let notes = amon_integration::install(IntegrationTarget::Claude).expect("install");
    assert!(
        notes.iter().all(|note| !note.contains("python3")),
        "{notes:?}"
    );

    let hook = config.join("hooks/amon-agent-state.sh");
    let script = std::fs::read_to_string(&hook).expect("hook installed");
    assert!(!script.contains("python"), "no interpreter: {script}");

    let (_, frames) = run_hook(
        &hook,
        &["session"],
        // What Claude Code actually passes a SessionStart hook.
        r#"{"hook_event_name":"SessionStart","session_id":"sess-42","transcript_path":"/tmp/t.jsonl","source":"startup"}"#,
        &config.join("hook.sock"),
    );
    let Method::AgentReportSession(report) = only_frame(&frames) else {
        panic!("expected a session report: {frames:?}");
    };
    assert_eq!(report.agent_id, "agent-under-test");
    assert_eq!(report.agent, "claude");
    assert_eq!(report.agent_session_id, "sess-42");
    assert_eq!(report.agent_session_path.as_deref(), Some("/tmp/t.jsonl"));
    assert_eq!(report.session_start_source.as_deref(), Some("startup"));
    assert_eq!(report.source, "amon:claude");

    let _ = std::fs::remove_dir_all(&config);
}

#[test]
fn the_claude_prompt_hook_reports_the_turn() {
    let config = scratch("prompt");
    set_env("CLAUDE_CONFIG_DIR", &config);
    amon_integration::install(IntegrationTarget::Claude).expect("install");

    let (_, frames) = run_hook(
        &config.join("hooks/amon-prompt-state.sh"),
        &[],
        r#"{"hook_event_name":"UserPromptSubmit","prompt":"fix the flaky test","session_id":"sess-42"}"#,
        &config.join("hook.sock"),
    );
    let Method::AgentReportActivity(report) = only_frame(&frames) else {
        panic!("expected an activity report: {frames:?}");
    };
    assert_eq!(report.text, "fix the flaky test");
    assert_eq!(report.kind, ActivityKind::Prompt);
    assert_eq!(report.agent_session_id.as_deref(), Some("sess-42"));

    let _ = std::fs::remove_dir_all(&config);
}

#[test]
fn the_codex_hook_reports_a_session_the_daemon_can_parse() {
    let config = scratch("codex");
    set_env("CODEX_HOME", &config);
    amon_integration::install(IntegrationTarget::Codex).expect("install");

    // codex keeps its hook beside its config rather than in a hooks/ dir.
    let (_, frames) = run_hook(
        &config.join("amon-agent-state.sh"),
        &["session"],
        // codex only reports when it knows the transcript path.
        r#"{"hook_event_name":"SessionStart","session_id":"codex-7","transcript_path":"/tmp/c.jsonl","source":"startup"}"#,
        &config.join("hook.sock"),
    );
    let Method::AgentReportSession(report) = only_frame(&frames) else {
        panic!("expected a session report: {frames:?}");
    };
    assert_eq!(report.agent, "codex");
    assert_eq!(report.agent_session_id, "codex-7");
    assert_eq!(report.source, "amon:codex");

    let _ = std::fs::remove_dir_all(&config);
}

#[test]
fn a_hook_that_has_nothing_to_report_sends_nothing() {
    let config = scratch("nothing");
    set_env("CLAUDE_CONFIG_DIR", &config);
    amon_integration::install(IntegrationTarget::Claude).expect("install");

    let (output, frames) = run_hook(
        &config.join("hooks/amon-agent-state.sh"),
        &["session"],
        r#"{"hook_event_name":"Stop","session_id":"sess-42"}"#,
        &config.join("hook.sock"),
    );
    assert!(frames.is_empty(), "{frames:?}");
    assert!(output.stdout.is_empty());

    let _ = std::fs::remove_dir_all(&config);
}

/// Every state hook amon supersedes installs as amon's own script - no
/// interpreter, herdr's markers kept, so the installed integration reads as
/// current - and a python3 script left from before reads as outdated.
#[test]
fn every_shell_hook_installs_without_python() {
    let home = scratch("home");
    set_env("HOME", &home);
    set_env("XDG_CONFIG_HOME", home.join(".config"));
    for key in [
        "CLAUDE_CONFIG_DIR",
        "CODEX_HOME",
        "COPILOT_HOME",
        "KIMI_HOME",
        "QWEN_HOME",
        "GROK_CONFIG_DIR",
        "CURSOR_CONFIG_DIR",
    ] {
        // SAFETY: as in set_env.
        unsafe { std::env::remove_var(key) };
    }

    let shell = [
        IntegrationTarget::Claude,
        IntegrationTarget::Codex,
        IntegrationTarget::Copilot,
        IntegrationTarget::Cursor,
        IntegrationTarget::Devin,
        IntegrationTarget::Droid,
        IntegrationTarget::Grok,
        IntegrationTarget::Kimi,
        IntegrationTarget::Mastracode,
        IntegrationTarget::Qodercli,
        IntegrationTarget::Qwen,
        IntegrationTarget::AntigravityCli,
    ];
    let path_of = |target| {
        amon_integration::statuses()
            .into_iter()
            .find(|status| status.target == target)
            .expect("a status for every target")
    };
    for target in shell {
        let status = path_of(target);
        // The installers want the agent's own directories to exist first.
        agent_dirs(&home, &status.path);

        let notes = amon_integration::install(target)
            .unwrap_or_else(|error| panic!("{target:?} installs: {error}"));
        assert!(
            notes.iter().all(|note| !note.contains("python3")),
            "{target:?}: {notes:?}"
        );
        let script = std::fs::read_to_string(&status.path).expect("installed");
        assert!(
            !script.contains("python"),
            "{target:?} needs no interpreter: {script}"
        );
        assert!(script.contains("hook input "), "{target:?}: {script}");
        assert_eq!(
            path_of(target).state,
            amon_integration::InstallState::Current,
            "{target:?} reads as current"
        );

        // An install from before - herdr's script, same markers - is outdated.
        let markers: String = script
            .lines()
            .filter(|line| line.contains("AMON_INTEGRATION_"))
            .map(|line| format!("{line}\n"))
            .collect();
        std::fs::write(
            &status.path,
            format!("#!/bin/sh\n{markers}python3 - <<'PY'\nPY\n"),
        )
        .expect("old script");
        assert_eq!(
            path_of(target).state,
            amon_integration::InstallState::Outdated,
            "{target:?} with python3 reads as outdated"
        );
    }

    let _ = std::fs::remove_dir_all(&home);
}

#[test]
fn the_antigravity_hook_always_answers_with_an_object() {
    let home = scratch("agy");
    set_env("HOME", &home);
    let path = amon_integration::statuses()
        .into_iter()
        .find(|status| status.target == IntegrationTarget::AntigravityCli)
        .expect("status")
        .path;
    agent_dirs(&home, &path);
    amon_integration::install(IntegrationTarget::AntigravityCli).expect("install");

    let (output, frames) = run_hook(
        &path,
        &["session"],
        r#"{"conversationId":"conv-1","transcriptPath":"/tmp/a.jsonl"}"#,
        &home.join("hook.sock"),
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout), "{}\n");
    let Method::AgentReportSession(report) = only_frame(&frames) else {
        panic!("expected a session report: {frames:?}");
    };
    assert_eq!(
        (report.agent.as_str(), report.agent_session_id.as_str()),
        ("agy", "conv-1")
    );

    // Outside amon it still answers, and reports nothing.
    let quiet = Command::new("sh")
        .arg(&path)
        .arg("session")
        .env_remove(amon_protocol::env::AMON_ENV)
        .stdin(Stdio::null())
        .output()
        .expect("hook runs");
    assert!(quiet.status.success());
    assert_eq!(String::from_utf8_lossy(&quiet.stdout), "{}\n");

    let _ = std::fs::remove_dir_all(&home);
}
