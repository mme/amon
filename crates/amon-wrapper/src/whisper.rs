//! Amon-to-amon frames inside the terminal stream.
//!
//! A wrapper on the far side of an ssh session has no socket that reaches this
//! machine, but its terminal bytes arrive faithfully. So registry traffic
//! rides the stream as private DCS sequences — `ESC P +amon;1; … ESC \` —
//! which the wrapper on this side strips back out before the real terminal or
//! the shadow terminal sees them. Terminals that have no amon in front of them
//! discard a well-formed DCS silently, which is what makes the one
//! unsolicited frame (the knock) safe to send.
//!
//! The payload of an event frame is a serialized [`Method`]. serde_json
//! escapes every control character in strings as `\u00XX`, so a raw ESC can
//! never appear inside a payload and the `ESC \` terminator is unambiguous by
//! construction.

use amon_protocol::Method;

/// Opens every whisper. The `1` is the envelope version, additive-forever
/// like `PROTOCOL_VERSION`: readers ignore frames they do not understand.
pub const PREFIX: &[u8] = b"\x1bP+amon;1;";
/// Ends every whisper: ST, as `ESC \`.
pub const ST: &[u8] = b"\x1b\\";

#[derive(Debug, Clone, PartialEq)]
pub enum WhisperFrame {
    /// The remote wrapper's probe: "is an amon listening on the far end?"
    Knock,
    /// The local wrapper's reply, sent down the input stream.
    Answer,
    /// The remote wrapper is going away; the row reverts.
    Bye,
    /// Registry traffic: `agent.register` or `agent.update`. Boxed because a
    /// register carries a whole entry, and the fixed frames carry nothing.
    Event(Box<Method>),
}

/// Encodes a frame as the bytes that ride the stream.
pub fn encode(frame: &WhisperFrame) -> Vec<u8> {
    let payload: Vec<u8> = match frame {
        WhisperFrame::Knock => b"knock".to_vec(),
        WhisperFrame::Answer => b"here".to_vec(),
        WhisperFrame::Bye => b"bye".to_vec(),
        WhisperFrame::Event(method) => serde_json::to_vec(method.as_ref()).unwrap_or_default(),
    };
    let mut bytes = Vec::with_capacity(PREFIX.len() + payload.len() + ST.len());
    bytes.extend_from_slice(PREFIX);
    bytes.extend_from_slice(&payload);
    bytes.extend_from_slice(ST);
    bytes
}

/// Decodes a payload — the bytes between [`PREFIX`] and [`ST`]. `None` for
/// anything unrecognized: an unknown fixed word, JSON this build cannot
/// parse, or no JSON at all. Discarding is the whole error handling; a
/// whisper from a newer amon must cost nothing but itself.
pub(crate) fn parse_payload(payload: &[u8]) -> Option<WhisperFrame> {
    match payload {
        b"knock" => Some(WhisperFrame::Knock),
        b"here" => Some(WhisperFrame::Answer),
        b"bye" => Some(WhisperFrame::Bye),
        _ => serde_json::from_slice::<Method>(payload)
            .ok()
            .map(|method| WhisperFrame::Event(Box::new(method))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use amon_protocol::{AgentEntry, AgentPatch, AgentState};

    fn test_entry() -> AgentEntry {
        AgentEntry {
            id: "a1".into(),
            agent: "claude".into(),
            state: AgentState::Working,
            state_since: 1,
            cwd: "/work".into(),
            pid: 42,
            args: vec!["claude".into()],
            hostname: "workstation".into(),
            started_at: 1,
            title: None,
            agent_session_id: None,
            agent_session_path: None,
            window: None,
            workspace: None,
            branch: None,
            focused: None,
            seen: None,
        }
    }

    #[test]
    fn frames_encode_inside_the_envelope() {
        assert_eq!(encode(&WhisperFrame::Knock), b"\x1bP+amon;1;knock\x1b\\");
        assert_eq!(encode(&WhisperFrame::Answer), b"\x1bP+amon;1;here\x1b\\");
        assert_eq!(encode(&WhisperFrame::Bye), b"\x1bP+amon;1;bye\x1b\\");
    }

    #[test]
    fn an_event_round_trips_through_its_payload() {
        let method = Method::AgentUpdate(AgentPatch::new("a1"));
        let encoded = encode(&WhisperFrame::Event(Box::new(method.clone())));
        let payload = &encoded[PREFIX.len()..encoded.len() - ST.len()];
        match parse_payload(payload) {
            Some(WhisperFrame::Event(parsed)) => assert_eq!(*parsed, method),
            other => panic!("expected the same event back, got {other:?}"),
        }
    }

    #[test]
    fn a_serialized_event_never_contains_a_raw_escape() {
        let mut entry = test_entry();
        entry.title = Some("\u{1b}]0;sneaky".into());
        let encoded = encode(&WhisperFrame::Event(Box::new(Method::AgentRegister(entry))));
        let payload = &encoded[PREFIX.len()..encoded.len() - ST.len()];
        assert!(
            !payload.contains(&0x1b),
            "JSON must escape control characters"
        );
    }

    #[test]
    fn garbage_payloads_parse_to_nothing() {
        assert!(parse_payload(b"nonsense").is_none());
        assert!(parse_payload(b"{\"method\":\"no-such\",\"params\":{}}").is_none());
        assert!(parse_payload(b"").is_none());
    }
}
