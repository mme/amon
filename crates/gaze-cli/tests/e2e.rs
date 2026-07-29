//! End-to-end tests against the real binary and the real socket.
//!
//! Everything here goes through the two public surfaces — the `gaze` command
//! and the daemon's protocol — so none of it constrains how the wrapper,
//! observer, or registry are put together internally.

mod harness;

use harness::{agent_named, path_str, read_to_string, state_of, Client, Sandbox};

/// A fake claude that announces work through the OSC title, the way the real
/// one does, then goes quiet.
const WORKING_THEN_IDLE: &str = r#"#!/bin/sh
printf '\033]0;\342\240\213 working\007'
echo "starting"
sleep "${1:-2}"
printf '\033]0;\342\234\263 done\007'
echo "finished"
sleep "${2:-2}"
"#;

#[test]
fn output_is_identical_to_running_the_agent_without_gaze() {
    let sandbox = Sandbox::new();
    // Control characters, an escape sequence, and wide glyphs: anything gaze
    // interpreted, buffered differently, or synthesized would show up here.
    let script = "#!/bin/sh\nprintf 'plain\\ttab\\r\\n\\033[1mbold\\033[0m ⠋ ✳\\n'\n";
    let agent = sandbox.fake_agent("claude", script);

    let through_gaze = sandbox.run(&[&path_str(&agent)]).stdout;
    // The same program on a bare PTY of the same size. Comparing against this
    // rather than against the literal string keeps the test honest about what
    // the terminal driver itself does (newline translation), while still
    // failing if gaze adds, drops, or reorders a single byte.
    let without_gaze = harness::run_on_bare_pty(&path_str(&agent));

    assert_eq!(
        String::from_utf8_lossy(&through_gaze),
        String::from_utf8_lossy(&without_gaze)
    );
}

#[test]
fn the_agents_exit_code_is_the_wrappers_exit_code() {
    let sandbox = Sandbox::new();
    let agent = sandbox.fake_agent("claude", "#!/bin/sh\nexit 42\n");

    let output = sandbox.run(&[&path_str(&agent)]);

    assert_eq!(output.status.code(), Some(42));
}

#[test]
fn arguments_after_the_agent_belong_to_the_agent() {
    let sandbox = Sandbox::new();
    let agent = sandbox.fake_agent("claude", "#!/bin/sh\necho \"got:$*\"\n");

    // --help and --json are gaze's own flags elsewhere; here they must be
    // passed straight through.
    let output = sandbox.run(&[&path_str(&agent), "--help", "--json", "extra"]);

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("got:--help --json extra"), "{stdout:?}");
}

#[test]
fn a_running_agent_is_visible_in_status() {
    let sandbox = Sandbox::new();
    let agent = sandbox.fake_agent("claude", WORKING_THEN_IDLE);
    let mut child = sandbox.spawn_agent(&[&path_str(&agent), "5", "5"]);

    let agents = sandbox.wait_for_status("the agent to register", |agents| {
        agent_named(agents, "claude").is_some()
    });

    let entry = agent_named(&agents, "claude").expect("registered");
    assert_eq!(
        entry["cwd"],
        serde_json::json!(path_str(&std::env::current_dir().unwrap()))
    );
    assert!(entry["pid"].as_u64().unwrap_or(0) > 0, "{entry:#?}");
    assert!(
        entry["args"].as_array().is_some_and(|args| args.len() == 3),
        "argv is recorded whole: {entry:#?}"
    );
    assert!(!entry["hostname"].as_str().unwrap_or("").is_empty());

    let _ = child.kill();
    let _ = child.wait();
}

#[test]
fn the_entry_disappears_when_the_agent_exits() {
    let sandbox = Sandbox::new();
    let agent = sandbox.fake_agent("claude", "#!/bin/sh\necho hi\nsleep 1\n");
    let mut child = sandbox.spawn_agent(&[&path_str(&agent)]);

    sandbox.wait_for_status("the agent to register", |agents| {
        agent_named(agents, "claude").is_some()
    });
    let _ = child.wait();

    sandbox.wait_for_status("the agent to disappear", |agents| {
        agent_named(agents, "claude").is_none()
    });
}

#[test]
fn state_reaches_status_from_the_shadow_terminal() {
    let sandbox = Sandbox::new();
    let agent = sandbox.fake_agent("claude", WORKING_THEN_IDLE);
    let mut child = sandbox.spawn_agent(&[&path_str(&agent), "3", "5"]);

    // The braille spinner in the title is claude's working marker; detection
    // has to see it through the shadow terminal, with no hooks involved.
    sandbox.wait_for_status("the agent to report working", |agents| {
        agent_named(agents, "claude").is_some_and(|agent| state_of(&agent) == "working")
    });

    sandbox.wait_for_status("the agent to report idle", |agents| {
        agent_named(agents, "claude").is_some_and(|agent| state_of(&agent) == "idle")
    });

    let _ = child.kill();
    let _ = child.wait();
}

#[test]
fn subscribers_are_told_when_agents_come_and_go() {
    let sandbox = Sandbox::new();
    // Make sure a daemon exists before subscribing to it.
    sandbox.run(&["status"]);

    let mut client = Client::new(sandbox.connect());
    client.send(r#"{"id":"1","method":"hello","params":{"role":"subscriber","protocol":1,"version":"test"}}"#);
    client.read_frame();
    client.send(r#"{"id":"2","method":"subscribe"}"#);
    client.read_frame();

    let agent = sandbox.fake_agent("claude", "#!/bin/sh\necho hi\nsleep 1\n");
    let mut child = sandbox.spawn_agent(&[&path_str(&agent)]);

    let connected = client.wait_for_event("agent_connected");
    assert_eq!(connected["params"]["agent"], serde_json::json!("claude"));

    let _ = child.wait();
    let disconnected = client.wait_for_event("agent_disconnected");
    assert_eq!(
        disconnected["params"]["id"], connected["params"]["id"],
        "the same agent that connected"
    );
}

#[test]
fn hook_reports_reach_the_registry() {
    let sandbox = Sandbox::new();
    // A fake agent that reports a session the way an installed hook would,
    // using the socket path and id gaze put in its environment.
    let agent = sandbox.fake_agent(
        "claude",
        r#"#!/bin/sh
printf '{"id":"h1","method":"agent.report_session","params":{"agent_id":"%s","source":"gaze:claude","agent":"claude","seq":1,"agent_session_id":"session-abc","agent_session_path":"/tmp/transcript.jsonl"}}\n' "$GAZE_AGENT_ID" \
  | timeout 5 socat - "UNIX-CONNECT:$GAZE_SOCKET_PATH" >/dev/null 2>&1
sleep 5
"#,
    );
    let mut child = sandbox.spawn_agent(&[&path_str(&agent)]);

    let agents = sandbox.wait_for_status("the hook's session report", |agents| {
        agent_named(agents, "claude")
            .is_some_and(|agent| agent["agent_session_id"] == serde_json::json!("session-abc"))
    });

    let entry = agent_named(&agents, "claude").expect("registered");
    assert_eq!(
        entry["agent_session_path"],
        serde_json::json!("/tmp/transcript.jsonl")
    );

    let _ = child.kill();
    let _ = child.wait();
}

#[test]
fn a_wrapper_survives_the_daemon_being_killed() {
    let sandbox = Sandbox::new();
    let agent = sandbox.fake_agent("claude", "#!/bin/sh\necho hi\nsleep 20\n");
    let mut child = sandbox.spawn_agent(&[&path_str(&agent)]);

    sandbox.wait_for_status("the agent to register", |agents| {
        agent_named(agents, "claude").is_some()
    });

    // Kill the daemon the rude way, as a crash would.
    let mut client = Client::new(sandbox.connect());
    client.send(r#"{"id":"1","method":"daemon.shutdown"}"#);
    let _ = client.read_frame();
    drop(client);

    // The agent is still running, and the wrapper re-registers it against the
    // daemon that the next command starts.
    sandbox.wait_for_status("the agent to come back", |agents| {
        agent_named(agents, "claude").is_some()
    });
    assert!(
        child.try_wait().expect("try_wait").is_none(),
        "the agent must not have died with the daemon"
    );

    let _ = child.kill();
    let _ = child.wait();
}

#[test]
fn status_says_so_when_nothing_is_running() {
    let sandbox = Sandbox::new();

    let output = sandbox.run(&["status"]);

    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("no agents running"));
}

#[test]
fn unknown_install_targets_are_rejected_with_the_list() {
    let sandbox = Sandbox::new();

    let output = sandbox.run(&["install", "notanagent"]);

    assert!(!output.status.success());
    let stderr = read_to_string(&output.stderr[..]);
    assert!(stderr.contains("unknown agent: notanagent"), "{stderr}");
    assert!(stderr.contains("claude"), "the list is offered: {stderr}");
}
