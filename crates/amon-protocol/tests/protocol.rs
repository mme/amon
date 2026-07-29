//! The protocol's promises, exercised through its public surface only.
//!
//! The compatibility tests matter most: they are what stops a future change
//! from quietly breaking a long-running wrapper talking to an upgraded daemon.

use amon_protocol::{
    AgentEntry, AgentPatch, AgentState, ErrorCode, Event, Hello, Method, Request, Response, Role,
    ServerFrame, PROTOCOL_VERSION,
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
        title: Some("amon.sh".into()),
        agent_session_id: None,
        agent_session_path: None,
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
    assert_eq!(frame, ServerFrame::Event(Event::Unknown));
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
        ServerFrame::Event(event)
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
    assert_eq!(entry.title.as_deref(), Some("amon.sh"), "title untouched");
    assert_eq!(entry.pid, 4242);

    assert!(!patch.apply(&mut entry), "reapplying changes nothing");
}

#[test]
fn status_sorts_the_agents_that_need_a_human_first() {
    let mut states = [
        AgentState::Idle,
        AgentState::Unknown,
        AgentState::Blocked,
        AgentState::Working,
    ];
    states.sort_by_key(|state| state.urgency());
    assert_eq!(
        states,
        [
            AgentState::Blocked,
            AgentState::Working,
            AgentState::Idle,
            AgentState::Unknown
        ]
    );
}
