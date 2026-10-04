//! What a hook's JSON says, read in Rust (issue #77; ADR-0020, a supersession).
//!
//! Every shell hook amon installs used to hand its stdin to `python3` to read
//! one JSON object and send one message to the wrapper - fifteen scripts, a
//! Python start-up on every prompt, and nothing at all reported on a machine
//! without Python. Each is now `amon hook input <hook> [action]`: the JSON on
//! stdin, the rules below, the message out. The rules are the scripts' own,
//! agent by agent - herdr's vendored state hooks for the session and state
//! reports, amon's prompt hooks for the turns - so a hook reports exactly what
//! it reported before, only without the interpreter.
//!
//! Pure: the environment and the clock come in as arguments, so each agent's
//! rules are tested against the payloads it sends.

use std::path::Path;

use amon_protocol::{ActivityKind, AgentState, Method, ReportActivity, ReportSession, ReportState};
use serde_json::{Map, Value};

/// What the hook process can see besides its stdin.
pub struct Context<'a> {
    pub agent_id: String,
    /// The report's sequence number: nanoseconds, as the scripts used.
    pub seq: u64,
    pub env: &'a dyn Fn(&str) -> Option<String>,
    /// Devin only: the working directory, and `devin list --format json`
    /// run there, for a session the hook payload does not name.
    pub cwd: &'a Path,
    pub devin_list: &'a dyn Fn(&Path) -> Option<String>,
}

/// The hooks amon reads, by the name the scripts pass.
pub const HOOKS: [&str; 15] = [
    "claude",
    "codex",
    "copilot",
    "cursor",
    "devin",
    "droid",
    "grok",
    "kimi",
    "mastracode",
    "qodercli",
    "qwen",
    "antigravity_cli",
    "claude-prompt",
    "codex-prompt",
    "grok-prompt",
];

/// The messages this hook invocation sends: none, or one.
pub fn reports(hook: &str, action: Option<&str>, input: &str, context: &Context) -> Vec<Method> {
    let payload = parse(input);
    let report = match hook {
        "claude" => claude(action, &payload, context),
        "codex" => codex(action, &payload, context),
        "copilot" => copilot(&payload, context),
        "cursor" => cursor(action, &payload, context),
        "devin" => devin(action, &payload, context),
        "droid" => droid(action, &payload, context),
        "grok" => grok(action, &payload, context),
        "kimi" => kimi(action, &payload, context),
        "mastracode" => mastracode(action, &payload, context),
        "qodercli" => qodercli(action, &payload, context),
        "qwen" => qwen(action, &payload, context),
        "antigravity_cli" => antigravity(action, &payload, context),
        "claude-prompt" => claude_prompt(&payload, context),
        "codex-prompt" => codex_prompt(&payload, context),
        "grok-prompt" => grok_prompt(&payload, context),
        _ => None,
    };
    report.into_iter().collect()
}

/// The hook's stdin as an object. Anything else - empty, broken, an array -
/// is an empty object, which is what every script fell back to.
fn parse(input: &str) -> Map<String, Value> {
    match serde_json::from_str::<Value>(input) {
        Ok(Value::Object(map)) => map,
        _ => Map::new(),
    }
}

/// A non-empty string field.
fn text<'a>(payload: &'a Map<String, Value>, key: &str) -> Option<&'a str> {
    payload
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
}

/// The first of several spellings that is a non-empty string.
fn first_text<'a>(payload: &'a Map<String, Value>, keys: &[&str]) -> Option<&'a str> {
    keys.iter().find_map(|key| text(payload, key))
}

/// Python's `str(x or "")` for the event name: absent, null or empty is "".
fn event_name(payload: &Map<String, Value>) -> String {
    match payload.get("hook_event_name") {
        Some(Value::String(name)) => name.clone(),
        Some(Value::Null) | None => String::new(),
        Some(Value::Bool(false)) => String::new(),
        Some(other) => other.to_string(),
    }
}

fn session(
    context: &Context,
    source: &str,
    agent: &str,
    session_id: &str,
    path: Option<&str>,
    start_source: Option<&str>,
) -> Option<Method> {
    Some(Method::AgentReportSession(ReportSession {
        agent_id: context.agent_id.clone(),
        source: source.into(),
        agent: agent.into(),
        seq: context.seq,
        agent_session_id: session_id.into(),
        agent_session_path: path.map(str::to_owned),
        session_start_source: start_source.map(str::to_owned),
    }))
}

fn state(
    context: &Context,
    source: &str,
    agent: &str,
    action: &str,
    session_id: Option<&str>,
) -> Option<Method> {
    let state = serde_json::from_value::<AgentState>(Value::String(action.into())).ok()?;
    Some(Method::AgentReportState(ReportState {
        agent_id: context.agent_id.clone(),
        source: source.into(),
        agent: agent.into(),
        state,
        seq: context.seq,
        message: None,
        agent_session_id: session_id.map(str::to_owned),
    }))
}

fn prompt(
    context: &Context,
    source: &str,
    agent: &str,
    text: &str,
    session_id: Option<&str>,
) -> Option<Method> {
    Some(Method::AgentReportActivity(ReportActivity {
        agent_id: context.agent_id.clone(),
        source: source.into(),
        agent: agent.into(),
        seq: context.seq,
        text: text.into(),
        kind: ActivityKind::Prompt,
        agent_session_id: session_id.map(str::to_owned),
    }))
}

// --- herdr's state hooks, agent by agent -----------------------------------

fn claude(action: Option<&str>, payload: &Map<String, Value>, context: &Context) -> Option<Method> {
    if action != Some("session") {
        return None;
    }
    // Cursor runs Claude's hooks too; those are Cursor's to report.
    if (context.env)("CURSOR_VERSION").is_some() || payload.contains_key("cursor_version") {
        return None;
    }
    let event = event_name(payload);
    if event != "SessionStart" {
        return None;
    }
    // A subagent's session is not the agent's.
    if payload.get("agent_id").is_some_and(truthy) {
        return None;
    }
    let session_id = text(payload, "session_id")?;
    let start = text(payload, "source");
    session(
        context,
        "amon:claude",
        "claude",
        session_id,
        text(payload, "transcript_path"),
        start,
    )
}

fn codex(action: Option<&str>, payload: &Map<String, Value>, context: &Context) -> Option<Method> {
    if action != Some("session") {
        return None;
    }
    let event = event_name(payload);
    if !event.is_empty() && event != "SessionStart" {
        return None;
    }
    let session_id = text(payload, "session_id");
    // No transcript, no session worth naming.
    payload
        .get("transcript_path")
        .and_then(Value::as_str)
        .filter(|path| !path.trim().is_empty())?;
    // A Codex started by another Codex inherits its thread; that one is not ours.
    if let Some(inherited) = (context.env)("CODEX_THREAD_ID").filter(|value| !value.is_empty()) {
        if Some(inherited.as_str()) != session_id {
            return None;
        }
    }
    let start = if event == "SessionStart" {
        text(payload, "source")
    } else {
        None
    };
    session(context, "amon:codex", "codex", session_id?, None, start)
}

fn copilot(payload: &Map<String, Value>, context: &Context) -> Option<Method> {
    let event = first_text(payload, &["hook_event_name", "hookEventName"]);
    match event {
        Some(event) => {
            let normal: String = event.chars().filter(|c| *c != '_' && *c != '-').collect();
            if normal.to_lowercase() != "sessionstart" {
                return None;
            }
        }
        None => {
            let other = payload.contains_key("prompt")
                || first_text(
                    payload,
                    &[
                        "tool_name",
                        "toolName",
                        "notification_type",
                        "notificationType",
                        "stop_reason",
                        "stopReason",
                        "reason",
                    ],
                )
                .is_some();
            if other {
                return None;
            }
        }
    }
    let session_id = first_text(payload, &["session_id", "sessionId"])?;
    session(context, "amon:copilot", "copilot", session_id, None, None)
}

fn cursor(action: Option<&str>, payload: &Map<String, Value>, context: &Context) -> Option<Method> {
    if action != Some("session") {
        return None;
    }
    let event = first_text(payload, &["hook_event_name", "hookEventName"]);
    if !matches!(event, None | Some("sessionStart")) {
        return None;
    }
    let session_id = first_text(
        payload,
        &[
            "session_id",
            "sessionId",
            "conversation_id",
            "conversationId",
        ],
    )?;
    session(context, "amon:cursor", "cursor", session_id, None, None)
}

fn devin(action: Option<&str>, payload: &Map<String, Value>, context: &Context) -> Option<Method> {
    if action != Some("session") {
        return None;
    }
    let direct = first_text(payload, &["session_id", "sessionId"]).map(str::to_owned);
    let session_id = match direct {
        Some(id) => id,
        None => {
            // Devin does not always say; `devin list` does, matched by folder -
            // except where the payload says it is too early for that to be ours.
            let event = payload
                .get("hook_event_name")
                .and_then(Value::as_str)
                .unwrap_or("");
            if event == "UserPromptSubmit"
                || (event == "SessionStart"
                    && payload.get("source").and_then(Value::as_str) == Some("startup"))
            {
                return None;
            }
            let project = (context.env)("DEVIN_PROJECT_DIR")
                .filter(|dir| !dir.is_empty())
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|| context.cwd.to_path_buf());
            let listing = match (context.env)("AMON_DEVIN_LIST_JSON") {
                Some(injected) => Some(injected),
                None => (context.devin_list)(&project),
            }?;
            let entries: Vec<Value> = serde_json::from_str(&listing).ok()?;
            let project = realpath(&project);
            entries.iter().find_map(|entry| {
                let id = entry
                    .get("id")
                    .and_then(Value::as_str)
                    .filter(|id| !id.is_empty())?;
                let dir = entry.get("working_directory").and_then(Value::as_str)?;
                (realpath(Path::new(dir)) == project).then(|| id.to_owned())
            })?
        }
    };
    session(context, "amon:devin", "devin", &session_id, None, None)
}

fn realpath(path: &Path) -> std::path::PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

fn droid(action: Option<&str>, payload: &Map<String, Value>, context: &Context) -> Option<Method> {
    if action != Some("session") {
        return None;
    }
    session(
        context,
        "amon:droid",
        "droid",
        text(payload, "session_id")?,
        None,
        None,
    )
}

fn grok(action: Option<&str>, payload: &Map<String, Value>, context: &Context) -> Option<Method> {
    if action != Some("session") {
        return None;
    }
    let event = first_text(payload, &["hook_event_name", "hookEventName"]);
    if !matches!(
        event,
        None | Some("session_start") | Some("SessionStart") | Some("sessionStart")
    ) {
        return None;
    }
    let session_id = (context.env)("GROK_SESSION_ID")
        .filter(|id| !id.is_empty())
        .or_else(|| first_text(payload, &["session_id", "sessionId"]).map(str::to_owned))?;
    session(context, "amon:grok", "grok", &session_id, None, None)
}

fn kimi(action: Option<&str>, payload: &Map<String, Value>, context: &Context) -> Option<Method> {
    let action = action?;
    let session_id = text(payload, "session_id");
    match action {
        "session" => session(
            context,
            "amon:kimi",
            "kimi",
            session_id?,
            None,
            Some("startup"),
        ),
        "working" | "blocked" | "idle" => state(context, "amon:kimi", "kimi", action, session_id),
        _ => None,
    }
}

fn mastracode(
    action: Option<&str>,
    payload: &Map<String, Value>,
    context: &Context,
) -> Option<Method> {
    let action = action?;
    let session_id = text(payload, "session_id");
    match action {
        "session" => session(
            context,
            "amon:mastracode",
            "mastracode",
            session_id?,
            None,
            Some("startup"),
        ),
        "working" | "blocked" | "idle" => {
            state(context, "amon:mastracode", "mastracode", action, session_id)
        }
        _ => None,
    }
}

fn qodercli(
    action: Option<&str>,
    payload: &Map<String, Value>,
    context: &Context,
) -> Option<Method> {
    if action != Some("session") {
        return None;
    }
    session(
        context,
        "amon:qodercli",
        "qodercli",
        text(payload, "session_id")?,
        None,
        None,
    )
}

fn qwen(action: Option<&str>, payload: &Map<String, Value>, context: &Context) -> Option<Method> {
    if action != Some("session") {
        return None;
    }
    let session_id = text(payload, "session_id")?;
    let start = payload
        .get("source")
        .and_then(Value::as_str)
        .filter(|source| {
            matches!(
                *source,
                "startup" | "resume" | "clear" | "compact" | "branch"
            )
        });
    session(context, "amon:qwen", "qwen", session_id, None, start)
}

fn antigravity(
    action: Option<&str>,
    payload: &Map<String, Value>,
    context: &Context,
) -> Option<Method> {
    if action != Some("session") {
        return None;
    }
    let session_id = text(payload, "conversationId")?;
    session(
        context,
        "amon:antigravity_cli",
        "agy",
        session_id,
        text(payload, "transcriptPath"),
        None,
    )
}

// --- amon's own prompt hooks ----------------------------------------------

fn claude_prompt(payload: &Map<String, Value>, context: &Context) -> Option<Method> {
    if event_name(payload) != "UserPromptSubmit" {
        return None;
    }
    let text_value = payload
        .get("prompt")
        .and_then(Value::as_str)
        .filter(|p| !p.trim().is_empty())?;
    prompt(
        context,
        "amon:claude",
        "claude",
        text_value,
        text(payload, "session_id"),
    )
}

fn codex_prompt(payload: &Map<String, Value>, context: &Context) -> Option<Method> {
    if event_name(payload) != "UserPromptSubmit" {
        return None;
    }
    let text_value = payload
        .get("prompt")
        .and_then(Value::as_str)
        .filter(|p| !p.trim().is_empty())?;
    prompt(
        context,
        "amon:codex",
        "codex",
        text_value,
        text(payload, "session_id"),
    )
}

fn grok_prompt(payload: &Map<String, Value>, context: &Context) -> Option<Method> {
    // grok spells its events and fields several ways; all of them count.
    let nonblank = |keys: &[&str]| {
        keys.iter().find_map(|key| {
            payload
                .get(*key)
                .and_then(Value::as_str)
                .filter(|value| !value.trim().is_empty())
        })
    };
    let event = nonblank(&["hook_event_name", "hookEventName"]).unwrap_or("");
    if !matches!(
        event,
        "user_prompt_submit" | "userPromptSubmit" | "UserPromptSubmit"
    ) {
        return None;
    }
    let text_value = nonblank(&["prompt", "userPrompt", "user_prompt", "message", "text"])?;
    let session_id = (context.env)("GROK_SESSION_ID")
        .filter(|id| !id.is_empty())
        .or_else(|| nonblank(&["session_id", "sessionId"]).map(str::to_owned));
    prompt(
        context,
        "amon:grok",
        "grok",
        text_value,
        session_id.as_deref(),
    )
}

/// Python truthiness for a JSON value.
fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_some_and(|n| n != 0.0),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(hook: &str, action: Option<&str>, input: &str, env: &[(&str, &str)]) -> Option<Method> {
        let env: Vec<(String, String)> = env
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        let lookup = move |key: &str| env.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone());
        let context = Context {
            agent_id: "a1".into(),
            seq: 7,
            env: &lookup,
            cwd: Path::new("/work"),
            devin_list: &|_| None,
        };
        let mut out = reports(hook, action, input, &context);
        assert!(out.len() <= 1);
        out.pop()
    }

    fn session_of(method: Option<Method>) -> ReportSession {
        match method {
            Some(Method::AgentReportSession(report)) => report,
            other => panic!("expected a session report, got {other:?}"),
        }
    }

    #[test]
    fn claude_reports_the_session_a_session_start_names() {
        let report = session_of(run(
            "claude",
            Some("session"),
            r#"{"hook_event_name":"SessionStart","session_id":"s1","transcript_path":"/t.jsonl","source":"resume"}"#,
            &[],
        ));
        assert_eq!(report.agent, "claude");
        assert_eq!(report.source, "amon:claude");
        assert_eq!(report.agent_session_id, "s1");
        assert_eq!(report.agent_session_path.as_deref(), Some("/t.jsonl"));
        assert_eq!(report.session_start_source.as_deref(), Some("resume"));
        assert_eq!(report.agent_id, "a1");
        assert_eq!(report.seq, 7);
    }

    #[test]
    fn claude_ignores_subagents_cursor_other_events_and_other_actions() {
        let start = r#"{"hook_event_name":"SessionStart","session_id":"s1"}"#;
        assert!(run("claude", Some("working"), start, &[]).is_none());
        assert!(run(
            "claude",
            Some("session"),
            r#"{"hook_event_name":"Stop","session_id":"s1"}"#,
            &[]
        )
        .is_none());
        assert!(run(
            "claude",
            Some("session"),
            r#"{"hook_event_name":"SessionStart","session_id":"s1","agent_id":"sub"}"#,
            &[]
        )
        .is_none());
        assert!(run("claude", Some("session"), start, &[("CURSOR_VERSION", "1")]).is_none());
        assert!(run(
            "claude",
            Some("session"),
            r#"{"hook_event_name":"SessionStart","session_id":"s1","cursor_version":"1"}"#,
            &[]
        )
        .is_none());
        assert!(run("claude", Some("session"), "not json", &[]).is_none());
    }

    #[test]
    fn codex_needs_a_transcript_and_its_own_thread() {
        let input = r#"{"hook_event_name":"SessionStart","session_id":"t1","transcript_path":"/x","source":"startup"}"#;
        let report = session_of(run("codex", Some("session"), input, &[]));
        assert_eq!(
            (
                report.agent_session_id.as_str(),
                report.session_start_source.as_deref()
            ),
            ("t1", Some("startup"))
        );
        assert!(
            run("codex", Some("session"), r#"{"session_id":"t1"}"#, &[]).is_none(),
            "no transcript"
        );
        assert!(
            run(
                "codex",
                Some("session"),
                input,
                &[("CODEX_THREAD_ID", "other")]
            )
            .is_none(),
            "inherited"
        );
        assert!(run(
            "codex",
            Some("session"),
            input,
            &[("CODEX_THREAD_ID", "t1")]
        )
        .is_some());
        // An event-less payload is accepted, without a start source.
        let bare = session_of(run(
            "codex",
            Some("session"),
            r#"{"session_id":"t1","transcript_path":"/x","source":"x"}"#,
            &[],
        ));
        assert_eq!(bare.session_start_source, None);
    }

    #[test]
    fn copilot_takes_any_spelling_of_session_start_and_refuses_other_payloads() {
        for event in [
            "SessionStart",
            "session_start",
            "session-start",
            "sessionStart",
        ] {
            let input = format!(r#"{{"hookEventName":"{event}","sessionId":"c1"}}"#);
            assert_eq!(
                session_of(run("copilot", None, &input, &[])).agent_session_id,
                "c1",
                "{event}"
            );
        }
        assert!(run(
            "copilot",
            None,
            r#"{"hook_event_name":"Stop","session_id":"c1"}"#,
            &[]
        )
        .is_none());
        assert!(run("copilot", None, r#"{"prompt":"hi","session_id":"c1"}"#, &[]).is_none());
        assert!(run(
            "copilot",
            None,
            r#"{"toolName":"x","session_id":"c1"}"#,
            &[]
        )
        .is_none());
        assert!(run("copilot", None, r#"{"session_id":"c1"}"#, &[]).is_some());
    }

    #[test]
    fn cursor_reads_its_conversation_ids() {
        assert_eq!(
            session_of(run(
                "cursor",
                Some("session"),
                r#"{"conversation_id":"k"}"#,
                &[]
            ))
            .agent_session_id,
            "k"
        );
        assert!(run(
            "cursor",
            Some("session"),
            r#"{"hook_event_name":"stop","session_id":"k"}"#,
            &[]
        )
        .is_none());
    }

    #[test]
    fn devin_falls_back_to_the_session_listed_for_its_folder() {
        let listing = r#"[{"id":"d0","working_directory":"/elsewhere"},{"id":"d1","working_directory":"/work"}]"#;
        let found = session_of(run(
            "devin",
            Some("session"),
            r#"{"hook_event_name":"Stop"}"#,
            &[("AMON_DEVIN_LIST_JSON", listing)],
        ));
        assert_eq!(found.agent_session_id, "d1");
        assert!(run(
            "devin",
            Some("session"),
            r#"{"hook_event_name":"UserPromptSubmit"}"#,
            &[("AMON_DEVIN_LIST_JSON", listing)]
        )
        .is_none());
        assert!(run(
            "devin",
            Some("session"),
            r#"{"hook_event_name":"SessionStart","source":"startup"}"#,
            &[("AMON_DEVIN_LIST_JSON", listing)]
        )
        .is_none());
        assert_eq!(
            session_of(run(
                "devin",
                Some("session"),
                r#"{"sessionId":"direct"}"#,
                &[]
            ))
            .agent_session_id,
            "direct"
        );
    }

    #[test]
    fn grok_prefers_its_environment_session() {
        let report = session_of(run(
            "grok",
            Some("session"),
            r#"{"hook_event_name":"session_start","session_id":"p"}"#,
            &[("GROK_SESSION_ID", "env")],
        ));
        assert_eq!(report.agent_session_id, "env");
        assert!(run(
            "grok",
            Some("session"),
            r#"{"hook_event_name":"stop","session_id":"p"}"#,
            &[]
        )
        .is_none());
    }

    #[test]
    fn kimi_and_mastracode_report_states_as_well_as_sessions() {
        for (hook, source) in [("kimi", "amon:kimi"), ("mastracode", "amon:mastracode")] {
            match run(hook, Some("blocked"), r#"{"session_id":"k1"}"#, &[]) {
                Some(Method::AgentReportState(report)) => {
                    assert_eq!(report.state, AgentState::Blocked);
                    assert_eq!(report.source, source);
                    assert_eq!(report.agent_session_id.as_deref(), Some("k1"));
                }
                other => panic!("{hook}: {other:?}"),
            }
            let start = session_of(run(hook, Some("session"), r#"{"session_id":"k1"}"#, &[]));
            assert_eq!(start.session_start_source.as_deref(), Some("startup"));
            assert!(run(hook, Some("session"), "{}", &[]).is_none());
            assert!(run(hook, Some("nonsense"), r#"{"session_id":"k1"}"#, &[]).is_none());
        }
    }

    #[test]
    fn qwen_keeps_only_known_start_sources() {
        assert_eq!(
            session_of(run(
                "qwen",
                Some("session"),
                r#"{"session_id":"q","source":"compact"}"#,
                &[]
            ))
            .session_start_source
            .as_deref(),
            Some("compact")
        );
        assert_eq!(
            session_of(run(
                "qwen",
                Some("session"),
                r#"{"session_id":"q","source":"weird"}"#,
                &[]
            ))
            .session_start_source,
            None
        );
    }

    #[test]
    fn antigravity_reports_as_agy() {
        let report = session_of(run(
            "antigravity_cli",
            Some("session"),
            r#"{"conversationId":"g","transcriptPath":"/p"}"#,
            &[],
        ));
        assert_eq!(
            (report.agent.as_str(), report.agent_session_path.as_deref()),
            ("agy", Some("/p"))
        );
    }

    #[test]
    fn droid_and_qodercli_report_a_named_session() {
        assert!(run("droid", Some("session"), r#"{"session_id":"r"}"#, &[]).is_some());
        assert!(run("droid", Some("working"), r#"{"session_id":"r"}"#, &[]).is_none());
        assert!(run("qodercli", Some("session"), r#"{"session_id":"q"}"#, &[]).is_some());
        assert!(run("qodercli", Some("session"), "{}", &[]).is_none());
        assert!(
            run("qodercli", None, r#"{"session_id":"q"}"#, &[]).is_none(),
            "session only"
        );
        assert!(
            run("cursor", None, r#"{"session_id":"k"}"#, &[]).is_none(),
            "session only"
        );
    }

    #[test]
    fn prompt_hooks_report_the_submitted_prompt_as_a_turn() {
        for (hook, input) in [
            (
                "claude-prompt",
                r#"{"hook_event_name":"UserPromptSubmit","prompt":"fix it","session_id":"s"}"#,
            ),
            (
                "codex-prompt",
                r#"{"hook_event_name":"UserPromptSubmit","prompt":"fix it","session_id":"s"}"#,
            ),
            (
                "grok-prompt",
                r#"{"hookEventName":"userPromptSubmit","userPrompt":"fix it","sessionId":"s"}"#,
            ),
        ] {
            match run(hook, None, input, &[]) {
                Some(Method::AgentReportActivity(report)) => {
                    assert_eq!(
                        (report.text.as_str(), report.kind),
                        ("fix it", ActivityKind::Prompt),
                        "{hook}"
                    );
                    assert_eq!(report.agent_session_id.as_deref(), Some("s"), "{hook}");
                }
                other => panic!("{hook}: {other:?}"),
            }
        }
        assert!(run(
            "claude-prompt",
            None,
            r#"{"hook_event_name":"UserPromptSubmit","prompt":"   "}"#,
            &[]
        )
        .is_none());
        assert!(run(
            "claude-prompt",
            None,
            r#"{"hook_event_name":"Stop","prompt":"x"}"#,
            &[]
        )
        .is_none());
        let grok_other_event = r#"{"hook_event_name":"session_start","prompt":"x"}"#;
        assert!(run("grok-prompt", None, grok_other_event, &[]).is_none());
        assert!(
            run("codex-prompt", None, r#"{"prompt":"x"}"#, &[]).is_none(),
            "no event, no turn"
        );
    }

    #[test]
    fn an_unknown_hook_reports_nothing() {
        assert!(run("nobody", Some("session"), r#"{"session_id":"x"}"#, &[]).is_none());
    }
}
