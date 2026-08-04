//! End-to-end tests against the real binary and the real socket.
//!
//! Everything here goes through the two public surfaces — the `amon` command
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
fn output_is_identical_to_running_the_agent_without_amon() {
    let sandbox = Sandbox::new();
    // Control characters, an escape sequence, and wide glyphs: anything amon
    // interpreted, buffered differently, or synthesized would show up here.
    let script = "#!/bin/sh\nprintf 'plain\\ttab\\r\\n\\033[1mbold\\033[0m ⠋ ✳\\n'\n";
    let agent = sandbox.fake_agent("claude", script);

    let through_amon = sandbox.run(&[&path_str(&agent)]).stdout;
    // The same program on a bare PTY of the same size. Comparing against this
    // rather than against the literal string keeps the test honest about what
    // the terminal driver itself does (newline translation), while still
    // failing if amon adds, drops, or reorders a single byte.
    let without_amon = harness::run_on_bare_pty(&path_str(&agent));

    assert_eq!(
        String::from_utf8_lossy(&through_amon),
        String::from_utf8_lossy(&without_amon)
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

    // --help and --json are amon's own flags elsewhere; here they must be
    // passed straight through.
    let output = sandbox.run(&[&path_str(&agent), "--help", "--json", "extra"]);

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("got:--help --json extra"), "{stdout:?}");
}

#[test]
fn a_signal_death_is_reproduced_not_translated() {
    use std::os::unix::process::ExitStatusExt;

    let sandbox = Sandbox::new();
    // The agent dies of SIGTERM (15). Bare, the caller would see a signal
    // death; through amon it must see exactly the same, not exit code 1.
    let agent = sandbox.fake_agent("claude", "#!/bin/sh\nkill -TERM $$\n");

    let output = sandbox.run(&[&path_str(&agent)]);

    assert_eq!(
        output.status.signal(),
        Some(15),
        "amon must die the way the agent died: {:?}",
        output.status
    );
}

#[test]
fn non_utf8_argv_reaches_the_agent_byte_exact() {
    use std::os::unix::ffi::OsStrExt;

    let sandbox = Sandbox::new();
    let agent = sandbox.fake_agent("claude", "#!/bin/sh\nprintf 'got:%s.' \"$1\"\n");

    // Valid Unix argv, invalid UTF-8. Bare execve passes it through; so must
    // amon (ADR-0006: the agent's argv is opaque).
    let arg = std::ffi::OsStr::from_bytes(b"bad-\xff-arg");
    let mut command = sandbox.command(&[]);
    command.arg(&agent).arg(arg);
    let output = command.output().expect("amon runs");

    assert!(output.status.success(), "{output:?}");
    let stdout = &output.stdout;
    assert!(
        stdout
            .windows(b"got:bad-\xff-arg.".len())
            .any(|window| window == b"got:bad-\xff-arg."),
        "the agent must receive the exact bytes: {stdout:?}"
    );
}

#[test]
fn a_signal_to_amon_is_forwarded_to_the_agent() {
    let sandbox = Sandbox::new();
    // The agent proves it received SIGTERM (not the SIGHUP of a collapsing
    // PTY) by exiting 7. The background sleep sheds its PTY fds so the exit
    // is seen promptly.
    let agent = sandbox.fake_agent(
        "claude",
        "#!/bin/sh\ntrap 'exit 7' TERM\nsleep 20 </dev/null >/dev/null 2>&1 &\nwait $!\n",
    );
    let mut child = sandbox.spawn_agent(&[&path_str(&agent)]);
    sandbox.wait_for_status("the agent to register", |agents| {
        agent_named(agents, "claude").is_some()
    });

    // Kill *amon*, the only pid a user sees.
    let _ = std::process::Command::new("kill")
        .args(["-TERM", &child.id().to_string()])
        .status();

    let status = child.wait().expect("amon exits");
    assert_eq!(
        status.code(),
        Some(7),
        "the agent's TERM handler ran, and its exit is amon's: {status:?}"
    );

    // The wrapper went through its normal shutdown, so its hook socket is gone.
    let leftovers = std::fs::read_dir(sandbox.runtime_path("amon/agents"))
        .map(|entries| entries.flatten().count())
        .unwrap_or(0);
    assert_eq!(leftovers, 0, "the hook socket must be unlinked on exit");
}

#[test]
fn an_equal_version_daemon_is_reported_truthfully() {
    let sandbox = Sandbox::new();
    sandbox.run(&["status"]);

    let output = sandbox.run(&["daemon"]);

    assert!(!output.status.success());
    let stderr = read_to_string(&output.stderr[..]);
    assert!(
        stderr.contains("same or newer"),
        "an equal-version daemon must not be called newer: {stderr}"
    );
}

#[test]
fn amons_own_subcommands_reject_unknown_flags() {
    let sandbox = Sandbox::new();

    // A typo like `--josn` must fail loudly, not silently print the default.
    let output = sandbox.run(&["status", "--definitely-bogus"]);

    assert!(
        !output.status.success(),
        "unknown flags on amon's own subcommands are errors: {:?}",
        String::from_utf8_lossy(&output.stdout)
    );
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
    // using the socket path and id amon put in its environment.
    let agent = sandbox.fake_agent(
        "claude",
        r#"#!/bin/sh
printf '{"id":"h1","method":"agent.report_session","params":{"agent_id":"%s","source":"amon:claude","agent":"claude","seq":1,"agent_session_id":"session-abc","agent_session_path":"/tmp/transcript.jsonl"}}\n' "$AMON_AGENT_ID" \
  | timeout 5 socat - "UNIX-CONNECT:$AMON_SOCKET_PATH" >/dev/null 2>&1
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
fn a_state_reports_session_id_also_reaches_the_registry() {
    let sandbox = Sandbox::new();
    // Hooks stamp the session id on every state report; if the one-shot
    // session report is missed, identity must still arrive this way.
    let agent = sandbox.fake_agent(
        "claude",
        r#"#!/bin/sh
printf '{"id":"h1","method":"agent.report_state","params":{"agent_id":"%s","source":"amon:claude","agent":"claude","state":"working","seq":1,"agent_session_id":"sess-from-state"}}\n' "$AMON_AGENT_ID" \
  | timeout 5 socat - "UNIX-CONNECT:$AMON_SOCKET_PATH" >/dev/null 2>&1
sleep 5
"#,
    );
    let mut child = sandbox.spawn_agent(&[&path_str(&agent)]);

    sandbox.wait_for_status("the state report's session id", |agents| {
        agent_named(agents, "claude")
            .is_some_and(|agent| agent["agent_session_id"] == serde_json::json!("sess-from-state"))
    });

    let _ = child.kill();
    let _ = child.wait();
}

#[test]
fn a_stale_state_report_cannot_regress_the_session_id() {
    let sandbox = Sandbox::new();
    // A delayed report from an older session arrives after a newer one; the
    // state machine rejects it, and the registry must not regress either.
    let agent = sandbox.fake_agent(
        "claude",
        r#"#!/bin/sh
{
  printf '{"id":"h1","method":"agent.report_state","params":{"agent_id":"%s","source":"amon:claude","agent":"claude","state":"working","seq":2,"agent_session_id":"sess-b"}}\n' "$AMON_AGENT_ID"
  printf '{"id":"h2","method":"agent.report_state","params":{"agent_id":"%s","source":"amon:claude","agent":"claude","state":"working","seq":1,"agent_session_id":"sess-a"}}\n' "$AMON_AGENT_ID"
} | timeout 5 socat - "UNIX-CONNECT:$AMON_SOCKET_PATH" >/dev/null 2>&1
sleep 6
"#,
    );
    let mut child = sandbox.spawn_agent(&[&path_str(&agent)]);

    sandbox.wait_for_status("the newer session id", |agents| {
        agent_named(agents, "claude")
            .is_some_and(|agent| agent["agent_session_id"] == serde_json::json!("sess-b"))
    });
    // Give the stale report every chance to have been processed, then make
    // sure it changed nothing.
    std::thread::sleep(std::time::Duration::from_millis(500));
    let agents = sandbox.status_json();
    let entry = agent_named(agents.as_array().unwrap_or(&vec![]), "claude").expect("registered");
    assert_eq!(entry["agent_session_id"], serde_json::json!("sess-b"));

    let _ = child.kill();
    let _ = child.wait();
}

#[test]
fn an_identity_less_state_report_does_not_starve_a_session_report() {
    let sandbox = Sandbox::new();
    // A state report without a session id must not advance identity ordering:
    // the session report that lost the race still carries the only identity.
    let agent = sandbox.fake_agent(
        "claude",
        r#"#!/bin/sh
{
  printf '{"id":"h1","method":"agent.report_state","params":{"agent_id":"%s","source":"amon:claude","agent":"claude","state":"working","seq":5}}\n' "$AMON_AGENT_ID"
  printf '{"id":"h2","method":"agent.report_session","params":{"agent_id":"%s","source":"amon:claude","agent":"claude","seq":4,"agent_session_id":"sess-late","agent_session_path":"/tmp/late.jsonl"}}\n' "$AMON_AGENT_ID"
} | timeout 5 socat - "UNIX-CONNECT:$AMON_SOCKET_PATH" >/dev/null 2>&1
sleep 6
"#,
    );
    let mut child = sandbox.spawn_agent(&[&path_str(&agent)]);

    let agents = sandbox.wait_for_status("the late session report's identity", |agents| {
        agent_named(agents, "claude")
            .is_some_and(|agent| agent["agent_session_id"] == serde_json::json!("sess-late"))
    });
    let entry = agent_named(&agents, "claude").expect("registered");
    assert_eq!(
        entry["agent_session_path"],
        serde_json::json!("/tmp/late.jsonl")
    );

    let _ = child.kill();
    let _ = child.wait();
}

#[test]
fn a_session_report_fills_the_path_for_the_current_session() {
    let sandbox = Sandbox::new();
    // The state report won the race and already set the id; the session
    // report for the same session must still land its path, whatever its seq.
    let agent = sandbox.fake_agent(
        "claude",
        r#"#!/bin/sh
{
  printf '{"id":"h1","method":"agent.report_state","params":{"agent_id":"%s","source":"amon:claude","agent":"claude","state":"working","seq":2,"agent_session_id":"sess-b"}}\n' "$AMON_AGENT_ID"
  printf '{"id":"h2","method":"agent.report_session","params":{"agent_id":"%s","source":"amon:claude","agent":"claude","seq":1,"agent_session_id":"sess-b","agent_session_path":"/tmp/b.jsonl"}}\n' "$AMON_AGENT_ID"
} | timeout 5 socat - "UNIX-CONNECT:$AMON_SOCKET_PATH" >/dev/null 2>&1
sleep 6
"#,
    );
    let mut child = sandbox.spawn_agent(&[&path_str(&agent)]);

    sandbox.wait_for_status("the session path to be filled in", |agents| {
        agent_named(agents, "claude").is_some_and(|agent| {
            agent["agent_session_id"] == serde_json::json!("sess-b")
                && agent["agent_session_path"] == serde_json::json!("/tmp/b.jsonl")
        })
    });

    let _ = child.kill();
    let _ = child.wait();
}

#[test]
fn a_session_report_reanchors_state_detection_to_the_new_session() {
    let sandbox = Sandbox::new();
    // The same process switches sessions (as `/clear` or `--resume` does):
    // state reported for the new session must be accepted, not rejected
    // because the machine is still anchored to the old one.
    let agent = sandbox.fake_agent(
        "claude",
        r#"#!/bin/sh
{
  printf '{"id":"h1","method":"agent.report_state","params":{"agent_id":"%s","source":"amon:claude","agent":"claude","state":"working","seq":1,"agent_session_id":"sess-a"}}\n' "$AMON_AGENT_ID"
  printf '{"id":"h2","method":"agent.report_session","params":{"agent_id":"%s","source":"amon:claude","agent":"claude","seq":2,"agent_session_id":"sess-b","session_start_source":"clear"}}\n' "$AMON_AGENT_ID"
  printf '{"id":"h3","method":"agent.report_state","params":{"agent_id":"%s","source":"amon:claude","agent":"claude","state":"blocked","seq":3,"agent_session_id":"sess-b"}}\n' "$AMON_AGENT_ID"
} | timeout 5 socat - "UNIX-CONNECT:$AMON_SOCKET_PATH" >/dev/null 2>&1
sleep 6
"#,
    );
    let mut child = sandbox.spawn_agent(&[&path_str(&agent)]);

    sandbox.wait_for_status("the new session's state to win", |agents| {
        agent_named(agents, "claude").is_some_and(|agent| {
            state_of(&agent) == "blocked"
                && agent["agent_session_id"] == serde_json::json!("sess-b")
        })
    });

    let _ = child.kill();
    let _ = child.wait();
}

#[test]
fn the_hook_subcommand_relays_a_session_report() {
    // What qodercli's installed hook (and the PowerShell ones) actually run:
    // `amon hook report-agent-session …` with the wrapper's environment.
    use amon_protocol::{Method, Request, Response};
    use std::os::unix::net::UnixListener;

    let sandbox = Sandbox::new();
    let socket = sandbox.runtime_path("hook.sock");
    let listener = UnixListener::bind(&socket).expect("bind wrapper socket");
    let collector = std::thread::spawn(move || {
        let (stream, _) = listener.accept().expect("hook connects");
        let mut reader = std::io::BufReader::new(stream.try_clone().expect("clone"));
        let mut line = String::new();
        std::io::BufRead::read_line(&mut reader, &mut line).expect("one frame");
        let mut write = &stream;
        let _ = std::io::Write::write_all(&mut write, Response::ack("hook-1").to_line().as_bytes());
        line
    });

    let status = std::process::Command::new(harness::AMON)
        .args([
            "hook",
            "report-agent-session",
            "agent-under-test",
            "--source",
            "amon:qodercli",
            "--agent",
            "qodercli",
            "--agent-session-id",
            "q-session-1",
            "--seq",
            "7",
        ])
        .env(amon_protocol::env::AMON_ENV, "1")
        .env(amon_protocol::env::SOCKET_PATH, &socket)
        .env(amon_protocol::env::AGENT_ID, "agent-under-test")
        .status()
        .expect("amon runs");
    assert!(status.success(), "the relay must never fail a hook");

    let line = collector.join().expect("collector");
    let request = Request::parse(line.trim()).expect("the wrapper can parse this frame");
    let Method::AgentReportSession(report) = request.method else {
        panic!("expected a session report, got {}", request.method.name());
    };
    assert_eq!(report.agent_id, "agent-under-test");
    assert_eq!(report.source, "amon:qodercli");
    assert_eq!(report.agent, "qodercli");
    assert_eq!(report.agent_session_id, "q-session-1");
    assert_eq!(report.seq, 7);
}

#[test]
fn the_hook_subcommand_relays_a_state_report() {
    // kimi's hook reports state transitions through the CLI relay too.
    use amon_protocol::{Method, Request, Response};
    use std::os::unix::net::UnixListener;

    let sandbox = Sandbox::new();
    let socket = sandbox.runtime_path("hook-state.sock");
    let listener = UnixListener::bind(&socket).expect("bind wrapper socket");
    let collector = std::thread::spawn(move || {
        let (stream, _) = listener.accept().expect("hook connects");
        let mut reader = std::io::BufReader::new(stream.try_clone().expect("clone"));
        let mut line = String::new();
        std::io::BufRead::read_line(&mut reader, &mut line).expect("one frame");
        let mut write = &stream;
        let _ = std::io::Write::write_all(&mut write, Response::ack("hook-1").to_line().as_bytes());
        line
    });

    let status = std::process::Command::new(harness::AMON)
        .args([
            "hook",
            "report-agent",
            "agent-under-test",
            "--source",
            "amon:kimi",
            "--agent",
            "kimi",
            "--state",
            "working",
            "--agent-session-id",
            "k-session-1",
            "--seq",
            "9",
        ])
        .env(amon_protocol::env::AMON_ENV, "1")
        .env(amon_protocol::env::SOCKET_PATH, &socket)
        .env(amon_protocol::env::AGENT_ID, "agent-under-test")
        .status()
        .expect("amon runs");
    assert!(status.success(), "the relay must never fail a hook");

    let line = collector.join().expect("collector");
    let request = Request::parse(line.trim()).expect("the wrapper can parse this frame");
    let Method::AgentReportState(report) = request.method else {
        panic!("expected a state report, got {}", request.method.name());
    };
    assert_eq!(report.agent, "kimi");
    assert_eq!(report.state, amon_protocol::AgentState::Working);
    assert_eq!(report.agent_session_id.as_deref(), Some("k-session-1"));
    assert_eq!(report.seq, 9);
}

#[test]
fn the_hook_subcommand_is_silent_when_amon_is_not_wrapping() {
    // Without the wrapper's environment the relay must cost the agent nothing:
    // no output, no failure — the same contract the hook scripts themselves keep.
    let output = std::process::Command::new(harness::AMON)
        .args([
            "hook",
            "report-agent-session",
            "a1",
            "--source",
            "amon:qodercli",
            "--agent",
            "qodercli",
            "--agent-session-id",
            "q1",
            "--seq",
            "1",
        ])
        .env_remove(amon_protocol::env::AMON_ENV)
        .env_remove(amon_protocol::env::SOCKET_PATH)
        .env_remove(amon_protocol::env::AGENT_ID)
        .output()
        .expect("amon runs");

    assert!(output.status.success());
    assert!(
        output.stdout.is_empty() && output.stderr.is_empty(),
        "{output:?}"
    );
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
fn a_graceful_shutdown_removes_the_socket_file() {
    let sandbox = Sandbox::new();
    sandbox.run(&["status"]);
    assert!(sandbox.daemon_socket().exists(), "daemon came up");

    let mut client = Client::new(sandbox.connect());
    client.send(r#"{"id":"1","method":"daemon.shutdown"}"#);
    let _ = client.read_frame();

    // The daemon acks before it unwinds, so give it a moment to finish.
    let start = std::time::Instant::now();
    while sandbox.daemon_socket().exists() {
        assert!(
            start.elapsed() < std::time::Duration::from_secs(5),
            "the socket file must not be left behind"
        );
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
}

/// An agent that echoes whatever it is sent, so a test can see exactly what
/// reached it. `stty raw` keeps the line discipline out of the way; `-echo`
/// means anything that comes back was echoed by the agent, not the terminal.
const ECHOES_ITS_INPUT: &str = r#"#!/bin/sh
stty raw -echo
while IFS= read -r line; do
  printf 'got[%s]\n' "$line"
done
"#;

#[test]
fn focus_reports_never_reach_an_agent_that_did_not_ask_for_them() {
    // Amon turns focus reporting on for its own sake (ADR-0007), so the
    // reports it caused must be taken back out of the agent's input.
    let sandbox = Sandbox::new();
    let agent = sandbox.fake_agent("claude", ECHOES_ITS_INPUT);
    let mut session = harness::PtySession::start(&sandbox, &[&path_str(&agent)]);

    session.send(b"\x1b[Ohello\r");
    let seen = session.wait_for_output(b"got[");

    let text = String::from_utf8_lossy(&seen);
    assert!(
        text.contains("got[hello]"),
        "typing still arrives: {text:?}"
    );
    assert!(
        !text.contains("got[\u{1b}[O"),
        "the focus report was swallowed: {text:?}"
    );

    session.kill();
}

#[test]
fn the_terminal_is_asked_for_focus_reports_and_put_back_afterwards() {
    let sandbox = Sandbox::new();
    let agent = sandbox.fake_agent("claude", "#!/bin/sh\necho ready\n");
    let mut session = harness::PtySession::start(&sandbox, &[&path_str(&agent)]);
    session.wait_for_output(b"ready");
    session.wait();
    // Give the drain thread its moment to see the last bytes.
    std::thread::sleep(std::time::Duration::from_millis(300));

    let seen = session.output();
    let text = String::from_utf8_lossy(&seen);
    assert!(
        text.contains("\u{1b}[?1004h"),
        "asked for reports: {text:?}"
    );
    assert!(
        text.contains("\u{1b}[?1004l"),
        "left the terminal as it found it: {text:?}"
    );
}

#[test]
fn looking_away_reaches_the_registry() {
    let sandbox = Sandbox::new();
    let agent = sandbox.fake_agent("claude", ECHOES_ITS_INPUT);
    let mut session = harness::PtySession::start(&sandbox, &[&path_str(&agent)]);

    // Launching from this view counts as looking at it.
    sandbox.wait_for_status("the agent to be focused", |agents| {
        agent_named(agents, "claude").is_some_and(|agent| agent["focused"] == true)
    });

    session.send(b"\x1b[O");
    let agents = sandbox.wait_for_status("focus to be lost", |agents| {
        agent_named(agents, "claude").is_some_and(|agent| agent["focused"] == false)
    });
    assert_eq!(
        agent_named(&agents, "claude").expect("registered")["seen"],
        serde_json::json!(true),
        "looking away does not unsee what was already seen"
    );

    session.kill();
}

/// An agent that turns focus reporting on and then off again, the way a TUI
/// does around its own lifetime. Turning it off takes amon's reporting with it.
const TOGGLES_FOCUS_REPORTING: &str = r#"#!/bin/sh
printf '\033[?1004h'
sleep 0.2
printf '\033[?1004l'
sleep 0.5
"#;

#[test]
fn an_agent_turning_focus_reporting_off_gets_it_put_back() {
    let sandbox = Sandbox::new();
    let agent = sandbox.fake_agent("claude", TOGGLES_FOCUS_REPORTING);
    let mut session = harness::PtySession::start(&sandbox, &[&path_str(&agent)]);

    // Checked while the agent is still running: amon disables the mode itself
    // on the way out, so the last word on a finished session is always `l`.
    // What matters is that reporting came back *during* the session.
    let start = std::time::Instant::now();
    let text = loop {
        let seen = session.output();
        let text = String::from_utf8_lossy(&seen).into_owned();
        if text.contains("\u{1b}[?1004l")
            && text.rfind("\u{1b}[?1004h") > text.rfind("\u{1b}[?1004l")
        {
            break text;
        }
        assert!(
            start.elapsed() < std::time::Duration::from_secs(10),
            "focus reporting was never restored after the agent disabled it: {text:?}"
        );
        std::thread::sleep(std::time::Duration::from_millis(25));
    };
    assert!(text.contains("\u{1b}[?1004l"), "the agent did disable it");

    session.kill();
}

#[test]
fn amon_writes_nothing_of_its_own_when_stdout_is_not_a_terminal() {
    // `amon agent > log` typed at a shell: stdin is a terminal, stdout is not.
    // The agent toggles the mode itself, which is what would otherwise draw
    // amon into answering — into the log.
    let sandbox = Sandbox::new();
    let agent = sandbox.fake_agent("claude", TOGGLES_FOCUS_REPORTING);

    let output = harness::run_with_terminal_stdin(&sandbox, &[&path_str(&agent)]);

    assert_eq!(
        String::from_utf8_lossy(&output),
        "\u{1b}[?1004h\u{1b}[?1004l",
        "the agent's own bytes, and not one of amon's"
    );
}

#[test]
fn status_says_so_when_nothing_is_running() {
    let sandbox = Sandbox::new();

    let output = sandbox.run(&["status"]);

    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("no agents running"));
}

/// Where Omarchy discovers third-party plugins, relative to the config dir.
const PLUGIN_DIR: &str = "omarchy/plugins/sh.amon.workspaces";

#[test]
fn installing_the_omarchy_widget_puts_it_where_the_shell_looks() {
    let sandbox = Sandbox::new();

    let output = sandbox.run(&["install", "omarchy"]);
    assert!(output.status.success(), "{output:?}");

    let plugin = sandbox.config_path(PLUGIN_DIR);
    let manifest = std::fs::read_to_string(plugin.join("manifest.json")).expect("a manifest");
    let manifest: serde_json::Value = serde_json::from_str(&manifest).expect("valid json");

    // The contract Omarchy loads a plugin by. A manifest it cannot read is the
    // one failure with no symptom: the widget just never appears.
    assert_eq!(manifest["id"], "sh.amon.workspaces");
    assert_eq!(manifest["schemaVersion"], 1);
    assert_eq!(manifest["kinds"], serde_json::json!(["bar-widget"]));
    let entry = manifest["entryPoints"]["barWidget"]
        .as_str()
        .expect("entry");
    assert!(
        plugin.join(entry).exists(),
        "the manifest's entry point must be installed: {entry}"
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("omarchy plugin enable sh.amon.workspaces")
            // Positioned against the built-in before it is removed: the left
            // section anchors on omarchy.workspaces, so removing it first
            // leaves the widget with nowhere to go but the centre.
            && stdout
                .contains("omarchy bar plugin add sh.amon.workspaces --after omarchy.workspaces"),
        "the user is told how to enable it: {stdout}"
    );
}

#[test]
fn installing_the_omarchy_widget_leaves_the_bar_config_alone() {
    // shell.json becomes the user's canonical file once they touch it, and
    // Omarchy does not deep-merge — so amon writing into it could silently
    // discard a layout they chose.
    let sandbox = Sandbox::new();
    let shell_json = sandbox.config_path("omarchy/shell.json");
    std::fs::create_dir_all(shell_json.parent().unwrap()).expect("config dir");
    std::fs::write(&shell_json, "{\"mine\":true}").expect("write shell.json");

    sandbox.run(&["install", "omarchy"]);

    assert_eq!(
        std::fs::read_to_string(&shell_json).expect("still there"),
        "{\"mine\":true}",
        "the user's bar config must be untouched"
    );
}

#[test]
fn reinstalling_restores_a_widget_someone_broke() {
    let sandbox = Sandbox::new();
    let plugin = sandbox.config_path(PLUGIN_DIR);

    sandbox.run(&["install", "omarchy"]);
    let widget = plugin.join("AgentStates.qml");
    let original = std::fs::read_to_string(&widget).expect("installed");
    std::fs::write(&widget, "corrupted").expect("corrupt it");

    sandbox.run(&["install", "omarchy"]);

    assert_eq!(
        std::fs::read_to_string(&widget).expect("still there"),
        original,
        "reinstalling rewrites every file, not just the missing ones"
    );
}

#[test]
fn a_widget_someone_broke_is_not_reported_current() {
    // The point of reporting currency is catching a widget that no longer
    // matches the daemon it parses; a version string alone cannot do that.
    let sandbox = Sandbox::new();
    sandbox.run(&["install", "omarchy"]);
    let widget = sandbox.config_path(PLUGIN_DIR).join("Workspaces.qml");
    std::fs::write(&widget, "not the widget we shipped").expect("corrupt it");

    let listed = read_to_string(&sandbox.run(&["integrations"]).stdout[..]);
    let line = listed
        .lines()
        .find(|line| line.starts_with("omarchy"))
        .unwrap_or_default();

    assert!(
        !line.contains("current"),
        "a broken install must not claim to be current: {line}"
    );
}

#[test]
fn uninstalling_removes_amons_files_and_nothing_else() {
    let sandbox = Sandbox::new();
    let plugin = sandbox.config_path(PLUGIN_DIR);
    sandbox.run(&["install", "omarchy"]);

    // Something the user put there. Whatever it is, it is not amon's to delete.
    let theirs = plugin.join("notes.md");
    std::fs::write(&theirs, "mine").expect("write");

    let output = sandbox.run(&["uninstall", "omarchy"]);

    assert!(output.status.success(), "{output:?}");
    for ours in ["manifest.json", "AgentStates.qml", "Workspaces.qml"] {
        assert!(!plugin.join(ours).exists(), "{ours} goes");
    }
    assert_eq!(
        std::fs::read_to_string(&theirs).expect("theirs survives"),
        "mine",
        "a file amon did not write survives, and keeps the directory alive"
    );
}

#[test]
fn someone_elses_plugin_under_the_same_id_is_left_alone() {
    let sandbox = Sandbox::new();
    let plugin = sandbox.config_path(PLUGIN_DIR);
    std::fs::create_dir_all(&plugin).expect("their dir");
    let theirs = r#"{"id":"someone.else"}"#;
    std::fs::write(plugin.join("manifest.json"), theirs).expect("their file");

    let install = sandbox.run(&["install", "omarchy"]);
    assert!(!install.status.success(), "install refuses");
    let uninstall = sandbox.run(&["uninstall", "omarchy"]);
    assert!(!uninstall.status.success(), "uninstall refuses");

    assert_eq!(
        std::fs::read_to_string(plugin.join("manifest.json")).expect("untouched"),
        theirs
    );
}

#[test]
fn a_symlinked_plugin_directory_is_left_alone() {
    // Dotfiles kept in a repository and linked into place. Writing through the
    // link would edit files in that repository; removing them would delete
    // them from it.
    let sandbox = Sandbox::new();
    let plugin = sandbox.config_path(PLUGIN_DIR);
    let elsewhere = sandbox.runtime_path("dotfiles-widget");
    std::fs::create_dir_all(&elsewhere).expect("their repo");
    std::fs::write(elsewhere.join("manifest.json"), "theirs").expect("their file");
    std::fs::create_dir_all(plugin.parent().unwrap()).expect("plugins dir");
    std::os::unix::fs::symlink(&elsewhere, &plugin).expect("link it into place");

    assert!(!sandbox.run(&["install", "omarchy"]).status.success());
    assert!(!sandbox.run(&["uninstall", "omarchy"]).status.success());

    assert_eq!(
        std::fs::read_to_string(elsewhere.join("manifest.json")).expect("untouched"),
        "theirs"
    );
    assert!(elsewhere.exists(), "their directory survives");
}

#[test]
fn a_symlinked_widget_file_is_never_written_through() {
    // The same hazard one level down: the directory is amon's, but a file in
    // it points into someone's repository.
    let sandbox = Sandbox::new();
    sandbox.run(&["install", "omarchy"]);
    let plugin = sandbox.config_path(PLUGIN_DIR);
    let theirs = sandbox.runtime_path("their-widget.qml");
    std::fs::write(&theirs, "their widget").expect("their file");
    std::fs::remove_file(plugin.join("Workspaces.qml")).expect("make room");
    std::os::unix::fs::symlink(&theirs, plugin.join("Workspaces.qml")).expect("link");

    assert!(!sandbox.run(&["install", "omarchy"]).status.success());

    assert_eq!(
        std::fs::read_to_string(&theirs).expect("untouched"),
        "their widget",
        "install must not follow a symlink and truncate what it points at"
    );
}

#[test]
fn an_install_that_died_half_way_can_be_finished() {
    // Assets are written before the manifest, so an interrupted install leaves
    // files with nothing identifying them. Refusing that directory would lock
    // amon out of its own half-finished work.
    let sandbox = Sandbox::new();
    let plugin = sandbox.config_path(PLUGIN_DIR);
    sandbox.run(&["install", "omarchy"]);
    std::fs::remove_file(plugin.join("manifest.json")).expect("kill it half way");

    let output = sandbox.run(&["install", "omarchy"]);

    assert!(output.status.success(), "{output:?}");
    assert!(plugin.join("manifest.json").exists(), "install completes");
}

#[test]
fn integrations_reports_the_desktop_widget_alongside_the_agents() {
    let sandbox = Sandbox::new();

    let before = read_to_string(&sandbox.run(&["integrations"]).stdout[..]);
    assert!(
        before.contains("omarchy") && before.contains("not installed"),
        "an uninstalled widget is listed as such: {before}"
    );

    sandbox.run(&["install", "omarchy"]);
    let after = read_to_string(&sandbox.run(&["integrations"]).stdout[..]);
    let line = after
        .lines()
        .find(|line| line.starts_with("omarchy"))
        .unwrap_or_default();
    assert!(line.contains("current"), "now current: {after}");
    // Agent integrations are still listed; the desktop one is an addition.
    assert!(after.contains("claude"), "{after}");
}

#[test]
fn unknown_install_targets_are_rejected_with_the_list() {
    let sandbox = Sandbox::new();

    let output = sandbox.run(&["install", "notanagent"]);

    assert!(!output.status.success());
    let stderr = read_to_string(&output.stderr[..]);
    assert!(stderr.contains("unknown target: notanagent"), "{stderr}");
    assert!(stderr.contains("claude"), "the list is offered: {stderr}");
    assert!(
        stderr.contains("omarchy"),
        "desktops are installable too, so they are listed: {stderr}"
    );
}

/// `amon install <agent>` refuses when the agent itself is not there, so a
/// test about aliases has to put its config directory in place first.
fn agent_is_installed(sandbox: &Sandbox, config: &str) {
    std::fs::create_dir_all(sandbox.home_path(config)).expect("agent config dir");
}

fn bashrc(sandbox: &Sandbox) -> String {
    std::fs::read_to_string(sandbox.home_path(".bashrc")).unwrap_or_default()
}

#[test]
fn installing_an_agent_aliases_its_own_name() {
    let sandbox = Sandbox::new();
    agent_is_installed(&sandbox, ".claude");

    let output = sandbox.run(&["install", "claude"]);

    assert!(output.status.success(), "{output:?}");
    let rc = bashrc(&sandbox);
    assert!(
        rc.contains("alias claude='amon claude'"),
        "typing claude has to run it under amon: {rc}"
    );
}

#[test]
fn aliasing_can_be_declined() {
    let sandbox = Sandbox::new();
    agent_is_installed(&sandbox, ".claude");

    sandbox.run(&["install", "claude", "--no-alias"]);

    assert!(
        !sandbox.home_path(".bashrc").exists(),
        "the shell config is not touched at all"
    );
}

#[test]
fn reinstalling_does_not_repeat_the_block() {
    let sandbox = Sandbox::new();
    agent_is_installed(&sandbox, ".claude");

    sandbox.run(&["install", "claude"]);
    sandbox.run(&["install", "claude"]);

    let rc = bashrc(&sandbox);
    assert_eq!(rc.matches("# >>> amon >>>").count(), 1, "{rc}");
    assert_eq!(rc.matches("alias claude=").count(), 1, "{rc}");
}

#[test]
fn a_second_agent_joins_the_same_block() {
    let sandbox = Sandbox::new();
    agent_is_installed(&sandbox, ".claude");
    agent_is_installed(&sandbox, ".codex");

    sandbox.run(&["install", "claude"]);
    sandbox.run(&["install", "codex"]);

    let rc = bashrc(&sandbox);
    assert_eq!(
        rc.matches("# >>> amon >>>").count(),
        1,
        "one block for all of them: {rc}"
    );
    assert!(rc.contains("alias claude='amon claude'"), "{rc}");
    assert!(rc.contains("alias codex='amon codex'"), "{rc}");
}

#[test]
fn uninstalling_one_agent_leaves_the_others_alias_alone() {
    let sandbox = Sandbox::new();
    agent_is_installed(&sandbox, ".claude");
    agent_is_installed(&sandbox, ".codex");
    sandbox.run(&["install", "claude"]);
    sandbox.run(&["install", "codex"]);

    let output = sandbox.run(&["uninstall", "claude"]);

    assert!(output.status.success(), "{output:?}");
    let rc = bashrc(&sandbox);
    assert!(!rc.contains("alias claude="), "its own line goes: {rc}");
    assert!(
        rc.contains("alias codex='amon codex'"),
        "the other stays: {rc}"
    );
}

#[test]
fn uninstalling_the_last_agent_takes_the_block_with_it() {
    let sandbox = Sandbox::new();
    agent_is_installed(&sandbox, ".claude");
    std::fs::write(sandbox.home_path(".bashrc"), "# theirs\nexport EDITOR=hx\n").expect("write");
    sandbox.run(&["install", "claude"]);

    sandbox.run(&["uninstall", "claude"]);

    assert_eq!(
        bashrc(&sandbox),
        "# theirs\nexport EDITOR=hx\n",
        "the file goes back to exactly what it was"
    );
}

#[test]
fn a_symlinked_shell_config_is_not_followed() {
    // A ~/.bashrc symlinked into a dotfiles checkout is a file in someone's
    // repository; writing through the link would edit it.
    let sandbox = Sandbox::new();
    agent_is_installed(&sandbox, ".claude");
    let real = sandbox.home_path("dotfiles-bashrc");
    std::fs::write(&real, "# theirs\n").expect("write");
    std::os::unix::fs::symlink(&real, sandbox.home_path(".bashrc")).expect("symlink");

    let output = sandbox.run(&["install", "claude"]);

    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        std::fs::read_to_string(&real).expect("still there"),
        "# theirs\n",
        "the file behind the symlink is untouched"
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("alias claude='amon claude'"),
        "and the user is told the line to add themselves: {stdout}"
    );
}
