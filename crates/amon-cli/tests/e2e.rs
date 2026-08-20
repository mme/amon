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
fn a_truncated_amon_block_is_never_rewritten() {
    // A fence missing its closing marker makes everything after it look like
    // amon's to rewrite; editing would erase the user's own lines. Setup must
    // refuse and say why, leaving the file byte-identical.
    let sandbox = Sandbox::new();
    sandbox.fake_agent("claude", "#!/bin/sh\n");
    agent_is_installed(&sandbox, ".claude");
    let damaged = "# >>> amon >>>\nalias claude='amon claude'\nexport THEIRS=1\n";
    std::fs::write(sandbox.home_path(".bashrc"), damaged).expect("damaged rc");

    let output = sandbox.run(&["setup", "claude"]);

    assert_eq!(
        std::fs::read_to_string(sandbox.home_path(".bashrc")).expect("still there"),
        damaged,
        "a file amon cannot parse is a file amon does not touch"
    );
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("missing its closing"),
        "and the user is told what to repair: {output:?}"
    );
}

#[test]
fn setup_without_a_terminal_names_the_flags() {
    // A script that forgot its flag gets told what to type, never a silently
    // assumed --all: bashrc edits from an implicit default is exactly the
    // surprise the rest of amon refuses (ADR-0010).
    let sandbox = Sandbox::new();

    let output = sandbox.run(&["setup"]);

    assert!(!output.status.success(), "{output:?}");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("--all"), "the fix is named: {stderr}");
}

#[test]
fn remove_without_a_terminal_names_the_flags() {
    let sandbox = Sandbox::new();

    let output = sandbox.run(&["remove"]);

    assert!(!output.status.success(), "{output:?}");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("--all"), "the fix is named: {stderr}");
}

#[test]
fn setup_all_covers_detected_agents_and_the_widget() {
    let sandbox = Sandbox::new();
    // A detected agent: its command on PATH, its config dir in place.
    sandbox.fake_agent("claude", "#!/bin/sh\n");
    agent_is_installed(&sandbox, ".claude");
    // An omarchy CLI to offer the widget row and record the enable.
    let record = sandbox.runtime_path("omarchy-calls");
    sandbox.fake_agent(
        "omarchy",
        &format!("#!/bin/sh\necho \"$*\" >> {}\n", path_str(&record)),
    );

    let output = sandbox.run(&["setup", "--all"]);

    assert!(output.status.success(), "{output:?}");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("✓ claude — hooks installed, aliased"),
        "{stdout}"
    );
    assert!(
        stdout.contains("✓ workspace switcher widget — installed and enabled"),
        "{stdout}"
    );
    assert!(
        bashrc(&sandbox).contains("alias claude='amon claude'"),
        "--all aliases by default"
    );
    let calls = std::fs::read_to_string(&record).expect("omarchy invoked");
    let list = calls
        .find("plugin list --json")
        .expect("discovery is checked first — the shell's watcher is debounced");
    let enable = calls
        .find("plugin enable sh.amon.workspaces")
        .expect("--all is the explicit go-ahead, so the enable runs");
    assert!(
        list < enable,
        "enable must wait for discovery, not race the watcher: {calls}"
    );
    assert!(
        !calls.contains("restart shell"),
        "a first install has no compiled copy to go stale: {calls}"
    );
}

/// Plants a widget that is installed but is not what this build ships — the
/// one state in which the running shell can be left drawing an old copy.
fn stale_widget_is_installed(sandbox: &Sandbox) {
    let plugin = sandbox.config_path(PLUGIN_DIR);
    std::fs::create_dir_all(&plugin).expect("plugin dir");
    std::fs::write(
        plugin.join("manifest.json"),
        r#"{"schemaVersion":1,"id":"sh.amon.workspaces","name":"Workspaces (amon)","version":"1.0.0","kinds":["bar-widget"],"entryPoints":{"barWidget":"Workspaces.qml"},"omarchy":{"clonedFrom":"omarchy.workspaces"}}"#,
    )
    .expect("manifest");
    std::fs::write(plugin.join("Workspaces.qml"), "// an older widget\n").expect("qml");
    std::fs::write(plugin.join("AgentStates.qml"), "// an older widget\n").expect("qml");
}

#[test]
fn replacing_a_stale_widget_restarts_the_bar() {
    // Asking the shell to rescan does not pick up QML that changed under an
    // already-loaded plugin, so without the restart a setup that reports
    // "installed and enabled" leaves the previous widget on screen — the claim
    // would be false in exactly the case it matters (desktop::restart_shell).
    let sandbox = Sandbox::new();
    stale_widget_is_installed(&sandbox);
    let record = sandbox.runtime_path("omarchy-calls");
    sandbox.fake_agent(
        "omarchy",
        &format!("#!/bin/sh\necho \"$*\" >> {}\n", path_str(&record)),
    );

    let output = sandbox.run(&["setup", "--all"]);

    assert!(output.status.success(), "{output:?}");
    let calls = std::fs::read_to_string(&record).expect("omarchy invoked");
    let enable = calls
        .find("plugin enable sh.amon.workspaces")
        .expect("the widget is still installed and enabled");
    let restart = calls
        .find("restart shell")
        .expect("and then made the one the bar draws");
    assert!(
        enable < restart,
        "restarting before the enable would reload the old registration: {calls}"
    );
    assert!(
        String::from_utf8_lossy(&output.stdout)
            .contains("✓ workspace switcher widget — updated and restarted"),
        "an update reads as one, so a blinking bar is accounted for: {output:?}"
    );
}

#[test]
fn installing_an_unchanged_widget_leaves_the_bar_alone() {
    // Same bytes rewritten: nothing on screen is stale, and a restart would
    // take the notifications and the OSD down to achieve nothing.
    let sandbox = Sandbox::new();
    let record = sandbox.runtime_path("omarchy-calls");
    sandbox.fake_agent(
        "omarchy",
        &format!("#!/bin/sh\necho \"$*\" >> {}\n", path_str(&record)),
    );

    sandbox.run(&["setup", "--all"]);
    std::fs::write(&record, "").expect("forget the first run");
    let output = sandbox.run(&["setup", "--all"]);

    assert!(output.status.success(), "{output:?}");
    let calls = std::fs::read_to_string(&record).expect("omarchy invoked");
    assert!(
        !calls.contains("restart shell"),
        "the second run replaced nothing: {calls}"
    );
}

#[test]
fn remove_all_takes_everything_back() {
    let sandbox = Sandbox::new();
    sandbox.fake_agent("claude", "#!/bin/sh\n");
    agent_is_installed(&sandbox, ".claude");
    sandbox.fake_agent("omarchy", "#!/bin/sh\nexit 0\n");
    assert!(sandbox.run(&["setup", "--all"]).status.success());

    let output = sandbox.run(&["remove", "--all"]);

    assert!(output.status.success(), "{output:?}");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("✓ claude — hooks removed, unaliased"),
        "{stdout}"
    );
    assert!(
        !sandbox
            .config_path(PLUGIN_DIR)
            .join("manifest.json")
            .exists(),
        "the widget goes too"
    );
    assert!(
        !bashrc(&sandbox).contains("alias claude"),
        "no alias may outlive the hooks it pointed at"
    );
}

#[test]
fn remove_all_does_not_claim_unaliased_for_a_stranded_block() {
    // A bashrc that later moved behind a symlink strands the alias block
    // where amon will not write; removal must say so, not claim "unaliased".
    let sandbox = Sandbox::new();
    sandbox.fake_agent("claude", "#!/bin/sh\n");
    agent_is_installed(&sandbox, ".claude");
    assert!(sandbox.run(&["setup", "claude"]).status.success());
    // Migrate the bashrc into a dotfiles checkout, block and all.
    let theirs = sandbox.runtime_path("dotfiles-bashrc");
    std::fs::rename(sandbox.home_path(".bashrc"), &theirs).expect("migrate");
    std::os::unix::fs::symlink(&theirs, sandbox.home_path(".bashrc")).expect("link");

    let output = sandbox.run(&["remove", "--all"]);

    assert!(output.status.success(), "{output:?}");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        !stdout.contains("unaliased"),
        "no claim about lines that are still there: {stdout}"
    );
    assert!(
        stdout.contains("remove the amon block yourself"),
        "the stranded block is pointed out: {stdout}"
    );
    assert!(
        std::fs::read_to_string(&theirs)
            .expect("untouched")
            .contains("alias claude"),
        "the symlink target must not be edited"
    );
}

#[test]
fn removing_one_agent_points_out_a_stranded_block() {
    // Targeted removal cannot edit a symlinked bashrc either; the alias it
    // leaves behind must be named, not silently skipped.
    let sandbox = Sandbox::new();
    sandbox.fake_agent("claude", "#!/bin/sh\n");
    agent_is_installed(&sandbox, ".claude");
    assert!(sandbox.run(&["setup", "claude"]).status.success());
    let theirs = sandbox.runtime_path("dotfiles-bashrc");
    std::fs::rename(sandbox.home_path(".bashrc"), &theirs).expect("migrate");
    std::os::unix::fs::symlink(&theirs, sandbox.home_path(".bashrc")).expect("link");

    let output = sandbox.run(&["remove", "claude"]);

    assert!(output.status.success(), "{output:?}");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("alias claude='amon claude'"),
        "the surviving lines are named: {stdout}"
    );
    assert!(
        !stdout.contains("amon block"),
        "removing one agent must not instruct deleting the whole block: {stdout}"
    );
}

#[test]
fn doctor_reports_aliases_bash_actually_reads() {
    // A symlinked bashrc is off-limits for editing, but its aliases are
    // active all the same; a diagnostic saying "none" would point the wrong
    // way.
    let sandbox = Sandbox::new();
    sandbox.fake_agent("claude", "#!/bin/sh\n");
    agent_is_installed(&sandbox, ".claude");
    assert!(sandbox.run(&["setup", "claude"]).status.success());
    let theirs = sandbox.runtime_path("dotfiles-bashrc");
    std::fs::rename(sandbox.home_path(".bashrc"), &theirs).expect("migrate");
    std::os::unix::fs::symlink(&theirs, sandbox.home_path(".bashrc")).expect("link");

    let stdout = read_to_string(&sandbox.run(&["doctor"]).stdout[..]);

    assert!(
        stdout.contains("alias claude='amon claude'"),
        "active aliases show even through a symlink: {stdout}"
    );
    assert!(
        stdout.contains("symlink"),
        "and the hands-off reason is stated: {stdout}"
    );
}

#[test]
fn remove_all_with_only_a_stranded_block_is_not_nothing() {
    // Orphaned hooks deleted by hand, bashrc behind a symlink: nothing amon
    // can edit remains, but "nothing to remove" would be a lie while the
    // alias still takes over the agent's name.
    let sandbox = Sandbox::new();
    sandbox.fake_agent("claude", "#!/bin/sh\n");
    agent_is_installed(&sandbox, ".claude");
    assert!(sandbox.run(&["setup", "claude"]).status.success());
    let theirs = sandbox.runtime_path("dotfiles-bashrc");
    std::fs::rename(sandbox.home_path(".bashrc"), &theirs).expect("migrate");
    std::os::unix::fs::symlink(&theirs, sandbox.home_path(".bashrc")).expect("link");
    std::fs::remove_dir_all(sandbox.home_path(".claude")).expect("orphan everything");

    let output = sandbox.run(&["remove", "--all"]);

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        !stdout.contains("nothing to remove"),
        "a stranded block is something: {stdout}"
    );
    assert!(
        stdout.contains("remove the amon block yourself"),
        "and the manual step is named: {stdout}"
    );
}

#[test]
fn doctor_tells_a_wedged_daemon_from_an_absent_one() {
    // A socket that accepts and then says nothing blocks on-demand starts;
    // calling that "not running" would send the user in exactly the wrong
    // direction.
    let sandbox = Sandbox::new();
    let socket_dir = sandbox.runtime_path(amon_protocol::paths::app_dir_name());
    std::fs::create_dir_all(&socket_dir).expect("socket dir");
    let _wedged = std::os::unix::net::UnixListener::bind(socket_dir.join("amond.sock"))
        .expect("bind a daemon-shaped socket that never answers");

    let output = sandbox.run(&["doctor"]);

    assert!(output.status.success(), "{output:?}");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("unresponsive"),
        "accepted-but-silent is its own diagnosis: {stdout}"
    );
    assert!(
        !stdout.contains("not running"),
        "and not misreported as absent: {stdout}"
    );
}

#[test]
fn remove_all_sweeps_up_an_orphaned_alias() {
    // An integration whose hook files were deleted by hand no longer shows as
    // installed, so no per-agent removal runs — but its alias is still in
    // bashrc, still taking over the agent's name. Removing everything means
    // exactly that.
    let sandbox = Sandbox::new();
    sandbox.fake_agent("claude", "#!/bin/sh\n");
    agent_is_installed(&sandbox, ".claude");
    assert!(sandbox.run(&["setup", "claude"]).status.success());
    std::fs::remove_dir_all(sandbox.home_path(".claude")).expect("orphan the alias");

    let output = sandbox.run(&["remove", "--all"]);

    assert!(output.status.success(), "{output:?}");
    assert!(
        !bashrc(&sandbox).contains("alias claude"),
        "an orphaned alias must not survive remove --all: {}",
        bashrc(&sandbox)
    );
}

#[test]
fn a_symlinked_bashrc_is_reported_not_claimed_aliased() {
    // alias::install returns Ok with manual instructions for a symlinked
    // bashrc; the apply report must pass those on, not say "aliased" about a
    // file that was deliberately left alone.
    let sandbox = Sandbox::new();
    sandbox.fake_agent("claude", "#!/bin/sh\n");
    agent_is_installed(&sandbox, ".claude");
    let theirs = sandbox.runtime_path("dotfiles-bashrc");
    std::fs::write(&theirs, "# theirs\n").expect("their file");
    std::os::unix::fs::symlink(&theirs, sandbox.home_path(".bashrc")).expect("link");

    let output = sandbox.run(&["setup", "--all"]);

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("manual step"),
        "the manual instructions are surfaced: {stdout}"
    );
    assert!(
        !stdout.contains("hooks installed, aliased"),
        "no success claim about an alias that was not written: {stdout}"
    );
    assert_eq!(
        std::fs::read_to_string(&theirs).expect("untouched"),
        "# theirs\n",
        "the symlink target must not be edited"
    );
}

#[test]
fn a_failed_widget_disable_is_not_reported_as_restored() {
    // Removal proceeds past a wedged `omarchy plugin disable`, but the report
    // must carry the manual fix-up instead of claiming the built-in came back.
    let sandbox = Sandbox::new();
    sandbox.fake_agent("omarchy", "#!/bin/sh\nexit 0\n");
    assert!(sandbox.run(&["setup", "--all"]).status.success());
    sandbox.fake_agent("omarchy", "#!/bin/sh\nexit 1\n");

    let output = sandbox.run(&["remove", "--all"]);

    assert!(output.status.success(), "{output:?}");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("omarchy plugin disable"),
        "the manual fix-up is visible: {stdout}"
    );
    assert!(
        !stdout.contains("restored"),
        "no restore claim when the disable failed: {stdout}"
    );
    assert!(
        !sandbox
            .config_path(PLUGIN_DIR)
            .join("manifest.json")
            .exists(),
        "removal still proceeds"
    );
}

#[test]
fn the_setup_screen_quits_without_changes() {
    let sandbox = Sandbox::new();
    sandbox.fake_agent("claude", "#!/bin/sh\n");
    agent_is_installed(&sandbox, ".claude");

    let mut session = harness::PtySession::start(&sandbox, &["setup"]);
    session.wait_for_output(b"Set up amon");
    session.send(b"q");
    session.wait();

    assert!(
        !bashrc(&sandbox).contains("alias claude"),
        "quitting must change nothing"
    );
}

#[test]
fn the_setup_screen_applies_the_preselection_on_enter() {
    // The dead-simple first run: everything detected is preselected, so the
    // whole setup is one Enter.
    let sandbox = Sandbox::new();
    sandbox.fake_agent("claude", "#!/bin/sh\n");
    agent_is_installed(&sandbox, ".claude");

    let mut session = harness::PtySession::start(&sandbox, &["setup"]);
    session.wait_for_output(b"Set up amon");
    session.send(b"\r");
    session.wait();

    let output = session.output();
    let text = String::from_utf8_lossy(&output);
    assert!(
        text.contains("✓ claude"),
        "the apply reports itself: {text}"
    );
    assert!(
        bashrc(&sandbox).contains("alias claude='amon claude'"),
        "interactive setup always aliases"
    );
}

#[test]
fn the_setup_screen_removes_what_gets_unchecked() {
    // The reconciler half: an installed row, unchecked, is removed on apply —
    // immediately, with no second confirmation (ADR-0010, deliberate).
    let sandbox = Sandbox::new();
    sandbox.fake_agent("claude", "#!/bin/sh\n");
    agent_is_installed(&sandbox, ".claude");
    assert!(sandbox.run(&["setup", "claude"]).status.success());
    assert!(bashrc(&sandbox).contains("alias claude"));

    let mut session = harness::PtySession::start(&sandbox, &["setup"]);
    session.wait_for_output(b"installed");
    session.send(b" \r"); // uncheck the first row, apply

    session.wait();
    let text = String::from_utf8_lossy(&session.output()).to_string();
    assert!(
        text.contains("hooks removed, unaliased"),
        "unchecking removes: {text}"
    );
    assert!(
        !bashrc(&sandbox).contains("alias claude"),
        "the alias goes with the hooks"
    );
}

#[test]
fn doctor_reports_the_desktop_widget_alongside_the_agents() {
    let sandbox = Sandbox::new();

    let before = read_to_string(&sandbox.run(&["doctor"]).stdout[..]);
    assert!(
        before.contains("omarchy") && before.contains("not installed"),
        "an uninstalled widget is listed as such: {before}"
    );

    // The widget comes from `--all` now; there is no per-target form for it.
    sandbox.fake_agent("omarchy", "#!/bin/sh\nexit 0\n");
    sandbox.run(&["setup", "--all"]);
    let after = read_to_string(&sandbox.run(&["doctor"]).stdout[..]);
    let line = after
        .lines()
        .find(|line| line.trim_start().starts_with("omarchy "))
        .unwrap_or_default();
    assert!(line.contains("current"), "now current: {after}");
    // Agent integrations are still listed; the desktop one is an addition.
    assert!(after.contains("claude"), "{after}");
}

#[test]
fn unknown_install_targets_are_rejected_with_the_list() {
    let sandbox = Sandbox::new();

    let output = sandbox.run(&["setup", "notanagent"]);

    assert!(!output.status.success());
    let stderr = read_to_string(&output.stderr[..]);
    assert!(stderr.contains("unknown target: notanagent"), "{stderr}");
    assert!(stderr.contains("claude"), "the list is offered: {stderr}");
    assert!(
        !stderr.contains("omarchy"),
        "a target is always an agent; the desktop is not one of them: {stderr}"
    );
}

/// `amon setup <agent>` refuses when the agent itself is not there, so a
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

    let output = sandbox.run(&["setup", "claude"]);

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

    sandbox.run(&["setup", "claude", "--no-alias"]);

    assert!(
        !sandbox.home_path(".bashrc").exists(),
        "the shell config is not touched at all"
    );
}

#[test]
fn reinstalling_does_not_repeat_the_block() {
    let sandbox = Sandbox::new();
    agent_is_installed(&sandbox, ".claude");

    sandbox.run(&["setup", "claude"]);
    sandbox.run(&["setup", "claude"]);

    let rc = bashrc(&sandbox);
    assert_eq!(rc.matches("# >>> amon >>>").count(), 1, "{rc}");
    assert_eq!(rc.matches("alias claude=").count(), 1, "{rc}");
}

#[test]
fn a_second_agent_joins_the_same_block() {
    let sandbox = Sandbox::new();
    agent_is_installed(&sandbox, ".claude");
    agent_is_installed(&sandbox, ".codex");

    sandbox.run(&["setup", "claude"]);
    sandbox.run(&["setup", "codex"]);

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
    sandbox.run(&["setup", "claude"]);
    sandbox.run(&["setup", "codex"]);

    let output = sandbox.run(&["remove", "claude"]);

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
    sandbox.run(&["setup", "claude"]);

    sandbox.run(&["remove", "claude"]);

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

    let output = sandbox.run(&["setup", "claude"]);

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

/// A `hyprctl` that records the expression it was told to dispatch and answers
/// the way the real one does — `ok` on stdout, and exit 0 either way.
fn fake_hyprctl(sandbox: &Sandbox, record: &std::path::Path, reply: &str) {
    sandbox.fake_agent(
        "hyprctl",
        &format!(
            "#!/bin/sh\nshift\necho \"$*\" >> {}\n{reply}\n",
            path_str(record)
        ),
    );
}

/// Registers an agent the daemon will report, over the same socket a wrapper
/// uses. The returned client has to stay alive: an entry lives exactly as long
/// as its connection.
fn register_agent(
    sandbox: &Sandbox,
    id: &str,
    workspace: &str,
    window: &str,
    state: &str,
    seen: bool,
) -> Client {
    // Something has to start the daemon, and `focus` deliberately will not:
    // a keybinding must not pay for a daemon launch.
    sandbox.run(&["status"]);
    let mut client = Client::new(sandbox.connect());
    client.send(
        r#"{"id":"h","method":"hello","params":{"role":"wrapper","protocol":1,"version":"test"}}"#,
    );
    client.send(&format!(
        r#"{{"id":"r","method":"agent.register","params":{{"id":"{id}","agent":"claude","state":"{state}","state_since":1,"cwd":"/","pid":1,"args":[],"hostname":"h","started_at":1,"window":"{window}","workspace":"{workspace}","seen":{seen}}}}}"#
    ));
    client
}

#[test]
fn focus_switches_the_workspace_when_no_agent_wants_you() {
    // The behaviour the Super+N binding replaced, and what every failure here
    // has to degrade to: no daemon, no agents, still a workspace switch.
    let sandbox = Sandbox::new();
    let record = sandbox.runtime_path("dispatches");
    fake_hyprctl(&sandbox, &record, "echo ok");

    let output = sandbox.run(&["focus", "4"]);

    assert!(output.status.success(), "{output:?}");
    let dispatched = std::fs::read_to_string(&record).expect("hyprctl was called");
    assert!(
        dispatched.contains("workspace = \"4\""),
        "the plain switch: {dispatched}"
    );
    assert_eq!(dispatched.lines().count(), 1, "and only once: {dispatched}");
}

#[test]
fn focus_lands_on_the_agent_that_wants_you_most() {
    // Two agents on one workspace: one still working, one finished and not yet
    // looked at. Done outranks working — the finished one is what is waiting
    // on a human — so that is where the jump goes.
    let sandbox = Sandbox::new();
    let record = sandbox.runtime_path("dispatches");
    fake_hyprctl(&sandbox, &record, "echo ok");

    let _working = register_agent(&sandbox, "w", "3", "aaa1", "working", true);
    let _done = register_agent(&sandbox, "d", "3", "bbb2", "idle", false);
    sandbox.wait_for_status("both agents to register", |agents| agents.len() == 2);

    let output = sandbox.run(&["focus", "3"]);

    assert!(output.status.success(), "{output:?}");
    let dispatched = std::fs::read_to_string(&record).expect("hyprctl was called");
    assert!(
        dispatched.contains("address:0xbbb2"),
        "the finished agent, not the working one: {dispatched}"
    );
    assert_eq!(
        dispatched.lines().count(),
        1,
        "one dispatch, so nothing flickers: {dispatched}"
    );
}

#[test]
fn focus_leaves_an_agent_at_rest_alone() {
    // An idle agent that has been seen asks for nothing. Stealing the focus
    // onto it would be amon interrupting on its own account.
    let sandbox = Sandbox::new();
    let record = sandbox.runtime_path("dispatches");
    fake_hyprctl(&sandbox, &record, "echo ok");

    let _resting = register_agent(&sandbox, "i", "5", "ccc3", "idle", true);
    sandbox.wait_for_status("the agent to register", |agents| agents.len() == 1);

    let output = sandbox.run(&["focus", "5"]);

    assert!(output.status.success(), "{output:?}");
    let dispatched = std::fs::read_to_string(&record).expect("hyprctl was called");
    assert!(
        dispatched.contains("workspace = \"5\"") && !dispatched.contains("address:"),
        "the plain switch: {dispatched}"
    );
}

#[test]
fn focus_still_switches_when_the_window_has_gone() {
    // The agent was there when the daemon answered and its window is not there
    // now. `hyprctl` reports that by printing a warning and exiting 0, so the
    // fallback cannot key off the exit status.
    let sandbox = Sandbox::new();
    let record = sandbox.runtime_path("dispatches");
    fake_hyprctl(
        &sandbox,
        &record,
        "case \"$*\" in *window*) echo 'warning: window not found';; *) echo ok;; esac",
    );

    let _blocked = register_agent(&sandbox, "b", "2", "ddd4", "blocked", true);
    sandbox.wait_for_status("the agent to register", |agents| agents.len() == 1);

    let output = sandbox.run(&["focus", "2"]);

    assert!(output.status.success(), "{output:?}");
    let dispatched = std::fs::read_to_string(&record).expect("hyprctl was called");
    let window = dispatched.find("address:0xddd4").expect("tried the agent");
    let workspace = dispatched
        .find("workspace = \"2\"")
        .expect("then went to the workspace anyway");
    assert!(window < workspace, "in that order: {dispatched}");
}

/// The config file, in this sandbox. `amon-dev` because tests are a debug
/// build, which is the same split the state and runtime directories make.
fn write_config(sandbox: &Sandbox, contents: &str) -> std::path::PathBuf {
    let path = sandbox.config_path(&format!(
        "{}/config.toml",
        amon_protocol::paths::app_dir_name()
    ));
    std::fs::create_dir_all(path.parent().expect("config dir")).expect("config dir");
    std::fs::write(&path, contents).expect("write config");
    path
}

/// Asks the daemon for the configuration over the socket a subscriber uses.
fn ask_for_config(sandbox: &Sandbox) -> serde_json::Value {
    let mut client = Client::new(sandbox.connect());
    client.send(
        r#"{"id":"h","method":"hello","params":{"role":"subscriber","protocol":1,"version":"test"}}"#,
    );
    client.send(r#"{"id":"c","method":"config"}"#);
    for _ in 0..8 {
        let frame = client.read_frame();
        if frame.get("id").and_then(|id| id.as_str()) == Some("c") {
            return frame;
        }
    }
    panic!("no answer to the config request");
}

#[test]
fn the_daemon_serves_the_config_file() {
    // The widget cannot parse TOML and never learns where the file is: it asks
    // the daemon, which is the only reader (ADR-0012).
    let sandbox = Sandbox::new();
    write_config(
        &sandbox,
        "[bar]\nframe_ms = 120\nstate_beats = 2\n\n[bar.glyphs]\nblocked = \"!\"\nworking = [\"a\", \"b\"]\n\n[sound]\nenabled = true\n",
    );
    sandbox.run(&["status"]);

    let frame = ask_for_config(&sandbox);

    let config = &frame["result"]["config"];
    assert_eq!(config["bar"]["frame_ms"], 120, "{frame}");
    assert_eq!(config["bar"]["state_beats"], 2, "{frame}");
    assert_eq!(config["bar"]["glyphs"]["blocked"], "!", "{frame}");
    assert_eq!(
        config["bar"]["glyphs"]["working"],
        serde_json::json!(["a", "b"]),
        "{frame}"
    );
    assert_eq!(config["sound"]["enabled"], true, "{frame}");
    // What the file does not say is absent rather than zero, which is what
    // lets the bar fall through to its own defaults per field.
    assert!(config["bar"]["marker_beats"].is_null(), "{frame}");
}

#[test]
fn no_config_file_is_not_an_error() {
    // The normal case on most machines: the daemon answers with an empty
    // configuration and the bar uses its own defaults throughout.
    let sandbox = Sandbox::new();
    sandbox.run(&["status"]);

    let frame = ask_for_config(&sandbox);

    assert!(frame["error"].is_null(), "{frame}");
    assert!(
        frame["result"]["config"]["bar"]["frame_ms"].is_null(),
        "{frame}"
    );
}

#[test]
fn editing_the_config_reaches_a_subscriber_without_a_restart() {
    // The whole point of the daemon owning the file: the bar reconfigures
    // under a running shell. Polled, so this takes a moment rather than being
    // instant (ADR-0012).
    let sandbox = Sandbox::new();
    write_config(&sandbox, "[bar]\nframe_ms = 100\n");
    sandbox.run(&["status"]);

    let mut client = Client::new(sandbox.connect());
    client.send(
        r#"{"id":"h","method":"hello","params":{"role":"subscriber","protocol":1,"version":"test"}}"#,
    );
    client.send(r#"{"id":"s","method":"subscribe"}"#);
    // Written after subscribing, so the event cannot be the one the daemon
    // sent on startup.
    std::thread::sleep(std::time::Duration::from_millis(1100));
    write_config(&sandbox, "[bar]\nframe_ms = 250\n");

    let event = client.wait_for_event("config_changed");

    assert_eq!(event["params"]["bar"]["frame_ms"], 250, "{event}");
}

#[test]
fn a_broken_config_keeps_the_last_good_one() {
    // Saving a file mid-edit must not blank the bar. The daemon keeps what it
    // had rather than reverting to defaults.
    let sandbox = Sandbox::new();
    write_config(&sandbox, "[bar]\nframe_ms = 140\n");
    sandbox.run(&["status"]);
    assert_eq!(
        ask_for_config(&sandbox)["result"]["config"]["bar"]["frame_ms"],
        140
    );

    write_config(&sandbox, "[bar]\nframe_ms = \"not a number\n");
    std::thread::sleep(std::time::Duration::from_millis(1400));

    let frame = ask_for_config(&sandbox);
    assert_eq!(
        frame["result"]["config"]["bar"]["frame_ms"], 140,
        "unparseable TOML must not revert the bar: {frame}"
    );
}

/// Where a shim lands inside a sandbox: `amon-dev` under a debug build, which
/// is what a test is.
fn shim_path(sandbox: &Sandbox, command: &str) -> std::path::PathBuf {
    sandbox.home_path(&format!(
        ".local/share/{}/shims/{command}",
        amon_protocol::paths::app_dir_name()
    ))
}

#[test]
fn setting_an_agent_up_shims_it_for_what_omarchy_launches() {
    // Omarchy launches agents through a terminal's -e, which execs the binary
    // directly and never reads a shell alias — so an agent that is set up needs
    // a stand-in on PATH as well (ADR-0013).
    let sandbox = Sandbox::new();
    sandbox.fake_agent("omarchy", "#!/bin/sh\nexit 0\n");
    sandbox.fake_agent("claude", "#!/bin/sh\n");
    agent_is_installed(&sandbox, ".claude");

    let output = sandbox.run(&["setup", "claude"]);

    assert!(output.status.success(), "{output:?}");
    let shim = shim_path(&sandbox, "claude");
    assert!(shim.is_file(), "a shim for the agent that was set up");
}

#[test]
fn removing_an_agent_takes_its_shim_with_it() {
    let sandbox = Sandbox::new();
    sandbox.fake_agent("omarchy", "#!/bin/sh\nexit 0\n");
    sandbox.fake_agent("claude", "#!/bin/sh\n");
    agent_is_installed(&sandbox, ".claude");
    sandbox.run(&["setup", "claude"]);
    assert!(shim_path(&sandbox, "claude").is_file());

    let output = sandbox.run(&["remove", "claude"]);

    assert!(output.status.success(), "{output:?}");
    assert!(
        !shim_path(&sandbox, "claude").exists(),
        "an agent amon no longer hooks must not keep taking over its name"
    );
}

#[test]
fn a_shim_actually_runs_the_agent_under_amon() {
    // The end-to-end claim: executing the shim the way Omarchy would — a bare
    // exec, no shell — gets the agent wrapped rather than run directly.
    let sandbox = Sandbox::new();
    sandbox.fake_agent("omarchy", "#!/bin/sh\nexit 0\n");
    sandbox.fake_agent("claude", "#!/bin/sh\necho \"agent saw: $*\"\n");
    agent_is_installed(&sandbox, ".claude");
    sandbox.run(&["setup", "claude"]);

    let shim = shim_path(&sandbox, "claude");
    let output = std::process::Command::new(&shim)
        .arg("--permission-mode")
        .arg("bypassPermissions")
        .env("XDG_RUNTIME_DIR", sandbox.runtime_path(""))
        .env("XDG_STATE_HOME", sandbox.runtime_path("state"))
        .env("HOME", sandbox.home_path(""))
        .env("XDG_CONFIG_HOME", sandbox.home_path(".config"))
        // The shim's own directory first, exactly as the session PATH will
        // have it — which is also what makes the recursion guard load-bearing.
        .env(
            "PATH",
            format!(
                "{}:{}",
                shim.parent().expect("shim dir").display(),
                sandbox.sandbox_path()
            ),
        )
        .output()
        .expect("the shim runs");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("agent saw: --permission-mode bypassPermissions"),
        "the agent ran, with its arguments untouched: {stdout}"
    );
    assert!(
        output.status.success(),
        "and exited cleanly rather than recursing: {output:?}"
    );
}

#[test]
fn a_shim_called_from_inside_amon_does_not_wrap_twice() {
    // `amon claude` typed in a terminal whose PATH carries the shim dir: amon
    // spawns `claude`, the lookup finds the shim, and without the guard that
    // would put a second amon around the same agent.
    let sandbox = Sandbox::new();
    sandbox.fake_agent("omarchy", "#!/bin/sh\nexit 0\n");
    sandbox.fake_agent(
        "claude",
        "#!/bin/sh\necho \"depth: ${AMON_AGENT_ID:-none}\"\n",
    );
    agent_is_installed(&sandbox, ".claude");
    sandbox.run(&["setup", "claude"]);

    let shim = shim_path(&sandbox, "claude");
    let output = std::process::Command::new(&shim)
        .env("XDG_RUNTIME_DIR", sandbox.runtime_path(""))
        .env("XDG_STATE_HOME", sandbox.runtime_path("state"))
        .env("HOME", sandbox.home_path(""))
        .env("XDG_CONFIG_HOME", sandbox.home_path(".config"))
        .env(
            "PATH",
            format!(
                "{}:{}",
                shim.parent().expect("dir").display(),
                sandbox.sandbox_path()
            ),
        )
        // Standing in for "amon is already wrapping this".
        .env("AMON_AGENT_ID", "pretend-wrapper")
        .output()
        .expect("the shim runs");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("depth: pretend-wrapper"),
        "the agent ran directly, keeping the id it was given: {stdout}"
    );
}

#[test]
fn doctor_names_each_shim_as_a_file() {
    // Same shape as an integration: what it is, then where it is — a shim is
    // the same kind of fact, a thing amon put somewhere you may want to look.
    let sandbox = Sandbox::new();
    sandbox.fake_agent("omarchy", "#!/bin/sh\nexit 0\n");
    sandbox.fake_agent("claude", "#!/bin/sh\n");
    agent_is_installed(&sandbox, ".claude");
    sandbox.run(&["setup", "claude"]);

    let stdout = read_to_string(&sandbox.run(&["doctor"]).stdout[..]);

    let line = stdout
        .lines()
        .find(|line| line.contains("session") && line.contains("claude"))
        .expect("the shim is reported");
    assert!(
        line.contains(&format!(
            "{}/shims/claude",
            amon_protocol::paths::app_dir_name()
        )),
        "with the file it actually is: {line}"
    );
}

#[test]
fn doctor_names_a_path_only_where_something_is() {
    // A not-installed row used to print where the hook *would* go, which reads
    // as a list of files — every one of them absent. There is no "where" for a
    // thing that does not exist.
    let sandbox = Sandbox::new();
    sandbox.fake_agent("claude", "#!/bin/sh\n");
    agent_is_installed(&sandbox, ".claude");
    sandbox.run(&["setup", "claude"]);

    let stdout = read_to_string(&sandbox.run(&["doctor"]).stdout[..]);

    let installed = stdout
        .lines()
        .find(|line| line.trim_start().starts_with("claude "))
        .expect("the agent that is set up");
    assert!(
        installed.contains(".claude/hooks/amon-agent-state.sh"),
        "an installed hook says where it is: {installed}"
    );

    let absent = stdout
        .lines()
        .find(|line| line.contains("not installed"))
        .expect("an agent that is not set up");
    assert!(
        !absent.contains('/'),
        "but a missing one names no file: {absent}"
    );
}

fn config_file(sandbox: &Sandbox) -> std::path::PathBuf {
    sandbox.config_path(&format!(
        "{}/config.toml",
        amon_protocol::paths::app_dir_name()
    ))
}

#[test]
fn setup_leaves_a_commented_config_behind() {
    // Not needed for anything to work — every setting has a default. It exists
    // so the settings can be found at all, which reading an ADR is not.
    let sandbox = Sandbox::new();
    sandbox.fake_agent("claude", "#!/bin/sh\n");
    agent_is_installed(&sandbox, ".claude");
    assert!(!config_file(&sandbox).exists());

    sandbox.run(&["setup", "claude"]);

    let written = std::fs::read_to_string(config_file(&sandbox)).expect("a config file");
    // Every line commented, so writing it changes nothing.
    for line in written.lines() {
        let line = line.trim();
        assert!(
            line.is_empty() || line.starts_with('#') || line.starts_with('['),
            "a starter config must configure nothing: {line}"
        );
    }
    assert!(
        written.contains("enabled = false"),
        "and show how to silence the sounds: {written}"
    );
    assert!(written.contains("frame_ms"), "{written}");
    assert!(written.contains("marker_beats"), "{written}");
}

#[test]
fn setup_never_rewrites_a_config_you_have_edited() {
    // The moment it exists it is the user's, and setup runs often.
    let sandbox = Sandbox::new();
    sandbox.fake_agent("claude", "#!/bin/sh\n");
    agent_is_installed(&sandbox, ".claude");
    let path = config_file(&sandbox);
    std::fs::create_dir_all(path.parent().expect("dir")).expect("dir");
    let mine = "[sound]\nenabled = false\n";
    std::fs::write(&path, mine).expect("write");

    sandbox.run(&["setup", "claude"]);
    sandbox.run(&["setup", "--all"]);

    assert_eq!(
        std::fs::read_to_string(&path).expect("still there"),
        mine,
        "byte-identical after two more setups"
    );
}

#[test]
fn sound_is_on_without_a_config_file() {
    // The default the daemon reports when there is nothing to read.
    let sandbox = Sandbox::new();
    sandbox.run(&["status"]);

    let frame = ask_for_config(&sandbox);

    assert_eq!(
        frame["result"]["config"]["sound"]["enabled"], true,
        "{frame}"
    );
}
