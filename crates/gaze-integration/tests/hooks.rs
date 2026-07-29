//! Conformance between the installed hook scripts and the protocol.
//!
//! The hooks are shell and Python, so nothing at compile time connects them to
//! the Rust wire types. These tests install a hook the way `gaze install` does,
//! run it the way the agent would, and parse what comes out of the socket with
//! the very types the daemon and wrapper use — which is a stronger check than
//! validating against the generated schema, because it is the same code path
//! that would reject the frame in production.

use std::io::{BufRead, BufReader};
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};

use gaze_protocol::{Method, Request};

/// Somewhere short enough for a unix socket path, unique per test.
fn scratch(name: &str) -> PathBuf {
    static NEXT: AtomicU32 = AtomicU32::new(0);
    let dir = std::env::temp_dir().join(format!(
        "gzh{}-{}-{name}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("scratch dir");
    dir
}

/// Installs an agent's integration into a throwaway config directory and
/// returns the hook script's path.
fn install_into(
    target: gaze_integration::IntegrationTarget,
    config_env: &str,
    dir: &Path,
) -> Vec<String> {
    // SAFETY: single-threaded test process; nextest gives each test its own.
    unsafe { std::env::set_var(config_env, dir) };
    gaze_integration::install(target).expect("install succeeds")
}

/// Runs a hook with the environment gaze gives an agent, and returns whatever
/// it sent to the socket.
fn capture_hook_frames(hook: &Path, args: &[&str], stdin_json: &str, socket: &Path) -> Vec<String> {
    let listener = UnixListener::bind(socket).expect("bind hook socket");

    let collector = std::thread::spawn(move || {
        let mut frames = Vec::new();
        if let Ok((stream, _)) = listener.accept() {
            for line in BufReader::new(stream).lines().map_while(Result::ok) {
                if !line.trim().is_empty() {
                    frames.push(line);
                }
            }
        }
        frames
    });

    let mut child = Command::new("sh")
        .arg(hook)
        .args(args)
        .env(gaze_protocol::env::GAZE_ENV, "1")
        .env(gaze_protocol::env::SOCKET_PATH, socket)
        .env(gaze_protocol::env::AGENT_ID, "agent-under-test")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("hook runs");

    use std::io::Write;
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(stdin_json.as_bytes())
        .expect("write hook input");
    let output = child.wait_with_output().expect("hook exits");
    assert!(
        output.status.success(),
        "hook failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    collector.join().expect("collector")
}

#[test]
fn the_claude_hook_reports_a_session_the_daemon_can_parse() {
    let config = scratch("claude");
    let messages = install_into(
        gaze_integration::IntegrationTarget::Claude,
        "CLAUDE_CONFIG_DIR",
        &config,
    );
    assert!(
        messages.iter().any(|message| message.contains("hook")),
        "install reports what it wrote: {messages:?}"
    );

    let hook = config.join("hooks/gaze-agent-state.sh");
    assert!(hook.exists(), "hook installed at {}", hook.display());

    let socket = config.join("hook.sock");
    let frames = capture_hook_frames(
        &hook,
        &["session"],
        // What Claude Code actually passes a SessionStart hook.
        r#"{"hook_event_name":"SessionStart","session_id":"sess-42","transcript_path":"/tmp/t.jsonl","source":"startup"}"#,
        &socket,
    );

    assert_eq!(
        frames.len(),
        1,
        "one report per hook invocation: {frames:?}"
    );
    let request = Request::parse(&frames[0]).expect("the daemon can parse this frame");
    let Method::AgentReportSession(report) = request.method else {
        panic!("expected a session report, got {}", request.method.name());
    };

    assert_eq!(report.agent_id, "agent-under-test");
    assert_eq!(report.agent, "claude");
    assert_eq!(report.agent_session_id, "sess-42");
    assert_eq!(report.agent_session_path.as_deref(), Some("/tmp/t.jsonl"));
    assert_eq!(report.source, "gaze:claude", "the herdr rename applied");

    let _ = std::fs::remove_dir_all(&config);
}

#[test]
fn hooks_stay_silent_unless_gaze_is_wrapping_the_agent() {
    // Installed hooks run on every agent session, including ones gaze knows
    // nothing about. Without the environment they must do nothing at all.
    let config = scratch("quiet");
    install_into(
        gaze_integration::IntegrationTarget::Claude,
        "CLAUDE_CONFIG_DIR",
        &config,
    );
    let hook = config.join("hooks/gaze-agent-state.sh");

    let output = Command::new("sh")
        .arg(&hook)
        .arg("session")
        .env_remove(gaze_protocol::env::GAZE_ENV)
        .env_remove(gaze_protocol::env::SOCKET_PATH)
        .env_remove(gaze_protocol::env::AGENT_ID)
        .stdin(Stdio::null())
        .output()
        .expect("hook runs");

    assert!(output.status.success(), "a hook must never fail its agent");
    assert!(
        output.stdout.is_empty() && output.stderr.is_empty(),
        "a hook must never print into the agent's session: {output:?}"
    );

    let _ = std::fs::remove_dir_all(&config);
}

#[test]
fn installing_twice_is_idempotent() {
    let config = scratch("twice");
    install_into(
        gaze_integration::IntegrationTarget::Claude,
        "CLAUDE_CONFIG_DIR",
        &config,
    );
    let settings = config.join("settings.json");
    let after_first = std::fs::read_to_string(&settings).expect("settings written");

    gaze_integration::install(gaze_integration::IntegrationTarget::Claude).expect("reinstall");
    let after_second = std::fs::read_to_string(&settings).expect("settings still there");

    assert_eq!(
        after_first, after_second,
        "reinstalling must not duplicate the hook registration"
    );

    let _ = std::fs::remove_dir_all(&config);
}

#[test]
fn uninstall_removes_what_install_added() {
    let config = scratch("uninstall");
    install_into(
        gaze_integration::IntegrationTarget::Claude,
        "CLAUDE_CONFIG_DIR",
        &config,
    );
    let hook = config.join("hooks/gaze-agent-state.sh");
    assert!(hook.exists());

    gaze_integration::uninstall(gaze_integration::IntegrationTarget::Claude).expect("uninstall");

    assert!(!hook.exists(), "the managed hook file is gone");
    let settings = std::fs::read_to_string(config.join("settings.json")).unwrap_or_default();
    assert!(
        !settings.contains("gaze-agent-state"),
        "and its registration: {settings}"
    );

    let _ = std::fs::remove_dir_all(&config);
}

#[test]
fn the_codex_hook_reports_a_session_the_daemon_can_parse() {
    let config = scratch("codex");
    install_into(
        gaze_integration::IntegrationTarget::Codex,
        "CODEX_HOME",
        &config,
    );

    // codex keeps its hook beside its config rather than in a hooks/ dir.
    let hook = config.join("gaze-agent-state.sh");
    assert!(hook.exists(), "hook installed at {}", hook.display());
    assert!(config.join("hooks.json").exists(), "and registered");
    assert!(config.join("config.toml").exists());

    let socket = config.join("hook.sock");
    let frames = capture_hook_frames(
        &hook,
        &["session"],
        // codex only reports when it knows the transcript path, so a test
        // input without one proves nothing.
        r#"{"hook_event_name":"SessionStart","session_id":"codex-7","transcript_path":"/tmp/c.jsonl","source":"startup"}"#,
        &socket,
    );

    assert_eq!(frames.len(), 1, "{frames:?}");
    let request = Request::parse(&frames[0]).expect("the daemon can parse this frame");
    let Method::AgentReportSession(report) = request.method else {
        panic!("expected a session report, got {}", request.method.name());
    };
    assert_eq!(report.agent, "codex");
    assert_eq!(report.agent_session_id, "codex-7");
    assert_eq!(report.source, "gaze:codex");

    let _ = std::fs::remove_dir_all(&config);
}

#[test]
fn the_pi_extension_installs_and_uninstalls() {
    // pi's integration is a TypeScript extension rather than a shell hook, so
    // there is nothing to execute here — what matters is that it lands in the
    // extension directory under gaze's name and leaves cleanly.
    let config = scratch("pi");
    install_into(
        gaze_integration::IntegrationTarget::Pi,
        "PI_CODING_AGENT_DIR",
        &config,
    );

    let extension = config.join("extensions/gaze-agent-state.ts");
    assert!(extension.exists(), "installed at {}", extension.display());
    let source = std::fs::read_to_string(&extension).expect("readable");
    assert!(
        source.contains("GAZE_SOCKET_PATH") && source.contains("agent.report_state"),
        "the extension speaks gaze's protocol"
    );

    gaze_integration::uninstall(gaze_integration::IntegrationTarget::Pi).expect("uninstall");
    assert!(!extension.exists(), "and is removed again");

    let _ = std::fs::remove_dir_all(&config);
}

#[test]
fn every_shipped_hook_starts_with_its_shebang() {
    // The agents execute these files directly, so a shebang anywhere but the
    // first line makes the hook unrunnable. The provenance header goes after
    // it, which is easy to get wrong when re-vendoring.
    let assets = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/vendor/integration/assets");
    let mut checked = 0;

    for entry in walk(&assets) {
        let Ok(contents) = std::fs::read_to_string(&entry) else {
            continue;
        };
        if !contents.contains("#!/") {
            continue;
        }
        checked += 1;
        assert!(
            contents.starts_with("#!"),
            "{} has a shebang that is not on line 1",
            entry.display()
        );
    }

    assert!(checked > 0, "found no executable hook assets to check");
}

fn walk(dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return files;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            files.extend(walk(&path));
        } else {
            files.push(path);
        }
    }
    files
}
