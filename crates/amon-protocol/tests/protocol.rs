//! The protocol's promises, exercised through its public surface only.
//!
//! The compatibility tests matter most: they are what stops a future change
//! from quietly breaking a long-running wrapper talking to an upgraded daemon.

use amon_protocol::{
    AgentEntry, AgentPatch, AgentState, ErrorCode, Event, Hello, HerdrInfo, Method, Request,
    Response, Role, ServerFrame, PROTOCOL_VERSION,
};

fn entry() -> AgentEntry {
    AgentEntry {
        id: "a1".into(),
        agent: "claude".into(),
        state: AgentState::Working,
        state_since: 1_700_000_000_000,
        cwd: "/home/mme/Projects/amon.sh".into(),
        pid: 4242,
        args: vec!["claude".into(), "--resume".into()],
        hostname: "workstation".into(),
        started_at: 1_699_999_999_000,
        agent_session_id: None,
        agent_session_path: None,
        window: Some("0x1a2b".into()),
        workspace: None,
        project: None,
        subpath: None,
        branch: None,
        focused: None,
        seen: None,
        herdr: None,
    }
}

#[test]
fn request_round_trips_through_a_line() {
    let request = Request::new(
        "req-1",
        Method::Hello(Hello {
            role: Role::Wrapper,
            protocol: PROTOCOL_VERSION,
            version: "0.1.0".into(),
        }),
    );

    let line = request.to_line();
    assert!(line.ends_with('\n'), "frames are newline delimited");
    assert_eq!(Request::parse(line.trim()).unwrap(), request);
}

#[test]
fn register_carries_the_whole_entry() {
    let request = Request::new("req-2", Method::AgentRegister(entry()));
    let parsed = Request::parse(request.to_line().trim()).unwrap();
    assert_eq!(parsed.method, Method::AgentRegister(entry()));
}

#[test]
fn methods_are_named_as_the_hooks_send_them() {
    // Vendored hook assets send these strings; renaming one silently breaks
    // every installed integration.
    assert_eq!(
        Method::AgentReportState(amon_protocol::ReportState {
            agent_id: "a1".into(),
            source: "amon:claude".into(),
            agent: "claude".into(),
            state: AgentState::Working,
            seq: 1,
            agent_session_id: None,
        })
        .name(),
        "agent.report_state"
    );
    assert_eq!(Method::Status.name(), "status");
}

#[test]
fn unknown_methods_are_rejected_but_still_answerable() {
    let error = Request::parse(r#"{"id":"req-3","method":"agent.teleport","params":{}}"#)
        .expect_err("unknown method should not parse");

    assert_eq!(
        error.id.as_deref(),
        Some("req-3"),
        "the id survives so the peer can be answered rather than dropped"
    );
    assert_eq!(error.error.code, ErrorCode::UnsupportedMethod);
}

#[test]
fn malformed_frames_report_no_id() {
    let error = Request::parse("not json at all").expect_err("garbage should not parse");
    assert_eq!(error.id, None);
    assert_eq!(error.error.code, ErrorCode::InvalidFrame);
}

#[test]
fn known_method_with_bad_params_is_distinguished_from_an_unknown_one() {
    let error = Request::parse(r#"{"id":"req-4","method":"agent.register","params":{"id":7}}"#)
        .expect_err("bad params should not parse");
    assert_eq!(error.error.code, ErrorCode::InvalidParams);
}

#[test]
fn unknown_fields_are_ignored() {
    // A newer wrapper adding a field must not break an older daemon.
    let line = r#"{"id":"req-5","method":"agent.update","params":{"id":"a1","state":"idle","mood":"cheerful"},"trailer":true}"#;
    let parsed = Request::parse(line).unwrap();
    let Method::AgentUpdate(patch) = parsed.method else {
        panic!("expected an update");
    };
    assert_eq!(patch.state, Some(AgentState::Idle));
}

#[test]
fn unknown_events_are_skipped_not_fatal() {
    // A subscriber built against an older amon meets an event from a newer one.
    let frame = ServerFrame::parse(r#"{"event":"agent_teleported","params":{"id":"a1"}}"#).unwrap();
    assert_eq!(frame, ServerFrame::Event(Box::new(Event::Unknown)));
}

#[test]
fn server_frames_split_into_responses_and_events() {
    let response = Response::ack("req-6");
    assert_eq!(
        ServerFrame::parse(response.to_line().trim()).unwrap(),
        ServerFrame::Response(response)
    );

    let event = Event::AgentDisconnected { id: "a1".into() };
    let line = serde_json::to_string(&event).unwrap();
    assert_eq!(
        ServerFrame::parse(&line).unwrap(),
        ServerFrame::Event(Box::new(event))
    );
}

#[test]
fn error_responses_carry_a_code_and_a_message() {
    let response = Response::err(
        "req-7",
        amon_protocol::Error::new(ErrorCode::UnknownAgent, "no such agent: a9"),
    );
    let ServerFrame::Response(parsed) = ServerFrame::parse(response.to_line().trim()).unwrap()
    else {
        panic!("expected a response");
    };
    let error = parsed.error.expect("error present");
    assert_eq!(error.code, ErrorCode::UnknownAgent);
    assert!(error.message.contains("a9"));
    assert!(parsed.result.is_none());
}

#[test]
fn a_patch_changes_only_what_it_names() {
    let mut entry = entry();
    let mut patch = AgentPatch::new("a1");
    patch.state = Some(AgentState::Blocked);
    patch.state_since = Some(1_700_000_005_000);

    assert!(patch.apply(&mut entry), "state changed");
    assert_eq!(entry.state, AgentState::Blocked);
    assert_eq!(entry.window.as_deref(), Some("0x1a2b"), "window untouched");
    assert_eq!(entry.pid, 4242);

    assert!(!patch.apply(&mut entry), "reapplying changes nothing");
}

#[test]
fn a_patch_moves_the_agent_and_everything_that_moved_with_it() {
    // An agent that walks into a worktree changes its directory, its Project
    // and its branch in one step. Applying them separately would let an entry
    // sit briefly with a branch from one checkout and a Project from another.
    let mut entry = entry();
    let mut patch = AgentPatch::new("a1");
    patch.cwd = Some("/home/mme/.cache/claude-worktrees/amon-readme".into());
    patch.project = Some(Some("amon.sh".into()));
    patch.subpath = Some(None);
    patch.branch = Some(Some("readme-herdr".into()));

    assert!(patch.apply(&mut entry), "moving is a change");
    assert_eq!(entry.cwd, "/home/mme/.cache/claude-worktrees/amon-readme");
    assert_eq!(entry.project.as_deref(), Some("amon.sh"));
    assert_eq!(entry.subpath, None);
    assert_eq!(entry.branch.as_deref(), Some("readme-herdr"));

    // And walking out of a repository clears them rather than leaving the last
    // one standing.
    let mut leaving = AgentPatch::new("a1");
    leaving.project = Some(None);
    leaving.branch = Some(None);
    assert!(leaving.apply(&mut entry), "leaving is a change");
    assert_eq!(entry.project, None);
    assert_eq!(entry.branch, None);
}

#[test]
fn a_patch_can_clear_a_field_rather_than_leave_it_stale() {
    // A window that closed must not leave its last address on display
    // forever. Absent means "leave alone"; null means "clear".
    let mut entry = entry();
    let mut patch = AgentPatch::new("a1");
    patch.window = Some(None);

    assert!(patch.apply(&mut entry), "clearing is a change");
    assert_eq!(entry.window, None);

    // The distinction must survive the wire: null is not the same as absent.
    let request = Request::new("req-8", Method::AgentUpdate(patch));
    let line = request.to_line();
    assert!(line.contains(r#""window":null"#), "{line:?}");
    let Method::AgentUpdate(parsed) = Request::parse(line.trim()).unwrap().method else {
        panic!("expected an update");
    };
    assert_eq!(parsed.window, Some(None), "null still means clear");

    let untouched =
        Request::parse(r#"{"id":"req-9","method":"agent.update","params":{"id":"a1"}}"#).unwrap();
    let Method::AgentUpdate(absent) = untouched.method else {
        panic!("expected an update");
    };
    assert_eq!(absent.window, None, "absent still means leave alone");
}

#[test]
fn a_patch_carries_where_the_agent_is_and_whether_it_was_seen() {
    let mut entry = entry();
    let mut patch = AgentPatch::new("a1");
    patch.window = Some(Some("5643b0cb1810".into()));
    patch.workspace = Some(Some("3".into()));
    patch.focused = Some(Some(true));
    patch.seen = Some(Some(true));

    assert!(patch.apply(&mut entry));
    assert_eq!(entry.window.as_deref(), Some("5643b0cb1810"));
    assert_eq!(entry.workspace.as_deref(), Some("3"));
    assert_eq!(entry.focused, Some(true));
    assert_eq!(entry.seen, Some(true));
    assert!(!patch.apply(&mut entry), "reapplying changes nothing");
}

#[test]
fn a_patch_can_clear_a_window_that_closed() {
    // Same three-way distinction the title has: a window that is gone must be
    // expressible as "no longer known", not left stale at its last value.
    let mut entry = entry();
    entry.window = Some("5643b0cb1810".into());
    entry.focused = Some(true);

    let mut patch = AgentPatch::new("a1");
    patch.window = Some(None);
    patch.focused = Some(None);

    assert!(patch.apply(&mut entry), "clearing is a change");
    assert_eq!(entry.window, None);
    assert_eq!(entry.focused, None);

    let line = Request::new("req-10", Method::AgentUpdate(patch)).to_line();
    assert!(line.contains(r#""window":null"#), "{line:?}");
    let Method::AgentUpdate(parsed) = Request::parse(line.trim()).unwrap().method else {
        panic!("expected an update");
    };
    assert_eq!(parsed.window, Some(None), "null still means clear");
    assert_eq!(parsed.focused, Some(None));
}

#[test]
fn an_older_wrapper_that_knows_no_window_leaves_it_alone() {
    // Forward compatibility, the same promise the other optional fields make:
    // omitting a field a newer daemon knows about must not clear it.
    let mut entry = entry();
    entry.window = Some("5643b0cb1810".into());
    entry.seen = Some(true);

    let update =
        Request::parse(r#"{"id":"req-11","method":"agent.update","params":{"id":"a1"}}"#).unwrap();
    let Method::AgentUpdate(patch) = update.method else {
        panic!("expected an update");
    };
    patch.apply(&mut entry);

    assert_eq!(entry.window.as_deref(), Some("5643b0cb1810"));
    assert_eq!(entry.seen, Some(true));
}

#[test]
fn status_sorts_the_agents_that_need_a_human_first() {
    // Ranked over entries, not states, because second place is not a state:
    // done is idle-and-unseen, and it outranks an agent still working.
    let at = |state, seen| AgentEntry {
        state,
        seen,
        ..entry()
    };
    let mut agents = [
        at(AgentState::Idle, Some(true)),
        at(AgentState::Unknown, None),
        at(AgentState::Blocked, None),
        at(AgentState::Working, None),
        at(AgentState::Idle, Some(false)),
    ];
    agents.sort_by_key(|agent| agent.attention());

    let ranked: Vec<_> = agents
        .iter()
        .map(|agent| (agent.state, agent.seen))
        .collect();
    assert_eq!(
        ranked,
        [
            (AgentState::Blocked, None),
            (AgentState::Idle, Some(false)),
            (AgentState::Working, None),
            (AgentState::Idle, Some(true)),
            (AgentState::Unknown, None),
        ]
    );
}

#[test]
fn only_an_agent_at_rest_is_not_worth_a_jump() {
    // What `amon focus` refuses to move the focus for. An idle agent that has
    // been seen asks for nothing; the same agent unseen is "done" and does.
    let at = |state, seen| AgentEntry {
        state,
        seen,
        ..entry()
    };
    assert!(at(AgentState::Blocked, None).wants_attention());
    assert!(at(AgentState::Working, None).wants_attention());
    assert!(at(AgentState::Idle, Some(false)).wants_attention());
    assert!(!at(AgentState::Idle, Some(true)).wants_attention());
    assert!(!at(AgentState::Idle, None).wants_attention());
    assert!(!at(AgentState::Unknown, None).wants_attention());
}

#[test]
fn a_herdr_entry_round_trips_and_a_plain_one_stays_bare() {
    let with_herdr = AgentEntry {
        id: "herdr:default:term_1".into(),
        herdr: Some(HerdrInfo {
            socket: "/home/mme/.config/herdr/herdr.sock".into(),
            session: None,
            pane: "w1:p2".into(),
        }),
        ..entry()
    };
    let json = serde_json::to_value(&with_herdr).unwrap();
    assert_eq!(json["herdr"]["pane"], "w1:p2");
    // The default session is absence, not an empty string, so consumers have
    // one spelling to check.
    assert!(json["herdr"].get("session").is_none());
    let back: AgentEntry = serde_json::from_value(json).unwrap();
    assert_eq!(back, with_herdr);

    // Absent from the wire when not set, so a consumer that predates the
    // field sees nothing new.
    let json = serde_json::to_value(entry()).unwrap();
    assert!(json.get("herdr").is_none());
}
