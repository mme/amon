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

/// What a chunk of the agent's output turned out to contain.
pub struct OutputScan {
    /// The output with whisper bytes removed — `None` when the chunk was
    /// untouched, so the caller forwards the original buffer as-is (the same
    /// convention as [`crate::focus::Scan::stripped`]).
    pub forwarded: Option<Vec<u8>>,
    /// Whisper frames completed in this chunk, in order.
    pub frames: Vec<WhisperFrame>,
}

/// A payload larger than this is consumed to its terminator but not kept —
/// the frame is dropped. Well over any real entry, small enough that a
/// hostile stream cannot make the wrapper hoard memory.
const MAX_PAYLOAD: usize = 64 * 1024;

/// Strips whispers out of the output stream, and only whispers.
///
/// Bytes are withheld from the output only while they could still be an amon
/// whisper: at the first byte that diverges from [`PREFIX`], everything
/// withheld is flushed back in original order and a foreign sequence passes
/// through byte-identical. Once the full prefix has matched, the sequence is
/// amon's — its bytes never reach the terminal, however it ends. A stream
/// that never terminates a confirmed whisper is consumed the way the real
/// terminal would consume an unterminated DCS.
///
/// State survives chunk splits at any byte position. Withheld prefix bytes
/// need no storing — they are always `PREFIX[..n]`, re-emitted from the
/// constant when a candidate that crossed a chunk boundary diverges.
#[derive(Default)]
pub struct OutputScanner {
    state: ScanState,
    payload: Vec<u8>,
    oversized: bool,
}

#[derive(Default, Clone, Copy, PartialEq)]
enum ScanState {
    #[default]
    Passthrough,
    /// This many bytes of [`PREFIX`] matched so far.
    Prefix(usize),
    Payload,
    /// Inside a confirmed payload, having just seen an ESC that may begin ST.
    PayloadEscape,
}

impl OutputScanner {
    pub fn scan(&mut self, chunk: &[u8]) -> OutputScan {
        let mut frames = Vec::new();
        // Lazily materialized: stays `None` while the output would be the
        // chunk itself. `copied` is where the pending passthrough run begins.
        let mut output: Option<Vec<u8>> = None;
        let mut copied = 0;
        // Where the current prefix candidate began in *this* chunk; `None`
        // when it was carried in from a previous chunk, whose forwarded bytes
        // already omitted it.
        let mut candidate_start: Option<usize> = match self.state {
            ScanState::Prefix(_) => None,
            _ => Some(0),
        };

        let mut index = 0;
        while index < chunk.len() {
            let byte = chunk[index];
            match self.state {
                ScanState::Passthrough => {
                    if byte == 0x1b {
                        self.state = ScanState::Prefix(1);
                        candidate_start = Some(index);
                    }
                }
                ScanState::Prefix(matched) => {
                    if byte == PREFIX[matched] {
                        if matched + 1 == PREFIX.len() {
                            // Confirmed ours: everything from the candidate on
                            // is withheld for good.
                            let out = output.get_or_insert_with(Vec::new);
                            // A carried candidate began at this chunk's start.
                            let candidate = candidate_start.unwrap_or(0);
                            out.extend_from_slice(&chunk[copied..candidate]);
                            copied = index + 1;
                            self.state = ScanState::Payload;
                            self.payload.clear();
                            self.oversized = false;
                        } else {
                            self.state = ScanState::Prefix(matched + 1);
                        }
                    } else {
                        // Not ours after all. A candidate carried from an
                        // earlier chunk was already withheld there and must be
                        // re-emitted; one from this chunk simply stays in the
                        // passthrough run.
                        if candidate_start.is_none() {
                            let out = output.get_or_insert_with(Vec::new);
                            out.extend_from_slice(&chunk[copied..index]);
                            out.extend_from_slice(&PREFIX[..matched]);
                            copied = index;
                        }
                        self.state = ScanState::Passthrough;
                        candidate_start = Some(index);
                        continue; // reprocess this byte — it may start anew
                    }
                }
                ScanState::Payload => {
                    if byte == 0x1b {
                        self.state = ScanState::PayloadEscape;
                    } else if self.payload.len() < MAX_PAYLOAD {
                        self.payload.push(byte);
                    } else {
                        self.oversized = true;
                    }
                }
                ScanState::PayloadEscape => {
                    self.state = ScanState::Passthrough;
                    if byte == b'\\' && !self.oversized {
                        frames.extend(parse_payload(&self.payload));
                    }
                    // Not ST: the sequence was ours and is corrupt. It is
                    // dropped whole — payload, escape, and this byte — the
                    // way the terminal would never have shown it either.
                    self.payload.clear();
                    copied = index + 1;
                    // Keep the withhold running: nothing between the confirmed
                    // prefix and here reaches the output.
                    output.get_or_insert_with(Vec::new);
                }
            }
            index += 1;
        }

        // End of chunk. A candidate still open in this chunk is withheld now;
        // payload bytes are already excluded by `copied`.
        match self.state {
            ScanState::Prefix(_) => {
                // The candidate's bytes in this chunk — from where it began,
                // or the whole chunk when it carried in — are withheld.
                let candidate = candidate_start.unwrap_or(0);
                let out = output.get_or_insert_with(Vec::new);
                out.extend_from_slice(&chunk[copied..candidate]);
            }
            ScanState::Payload | ScanState::PayloadEscape => {
                // Payload bytes are already excluded; make the withhold
                // visible even when nothing else was copied this chunk.
                output.get_or_insert_with(Vec::new);
            }
            ScanState::Passthrough => {
                if let Some(out) = output.as_mut() {
                    out.extend_from_slice(&chunk[copied..]);
                }
            }
        }

        OutputScan {
            forwarded: output,
            frames,
        }
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

    #[test]
    fn plain_output_is_left_alone() {
        let mut scanner = OutputScanner::default();
        let scan = scanner.scan(b"hello \x1b[1mworld\x1b[0m");
        assert!(
            scan.forwarded.is_none(),
            "an untouched chunk stays borrowed"
        );
        assert!(scan.frames.is_empty());
    }

    #[test]
    fn a_whisper_is_stripped_and_parsed() {
        let mut scanner = OutputScanner::default();
        let mut bytes = b"before".to_vec();
        bytes.extend_from_slice(&encode(&WhisperFrame::Knock));
        bytes.extend_from_slice(b"after");
        let scan = scanner.scan(&bytes);
        assert_eq!(scan.forwarded.unwrap(), b"beforeafter");
        assert_eq!(scan.frames, vec![WhisperFrame::Knock]);
    }

    #[test]
    fn a_whisper_split_across_chunks_is_still_one_frame() {
        let encoded = encode(&WhisperFrame::Bye);
        for split in 1..encoded.len() {
            let mut scanner = OutputScanner::default();
            let first = scanner.scan(&encoded[..split]);
            let second = scanner.scan(&encoded[split..]);
            let forwarded: Vec<u8> = first
                .forwarded
                .unwrap_or_default()
                .into_iter()
                .chain(second.forwarded.unwrap_or_default())
                .collect();
            assert_eq!(forwarded, b"", "split at {split} leaked bytes");
            let frames: Vec<_> = first.frames.into_iter().chain(second.frames).collect();
            assert_eq!(frames, vec![WhisperFrame::Bye], "split at {split}");
        }
    }

    #[test]
    fn a_foreign_dcs_passes_through_byte_identical() {
        let mut scanner = OutputScanner::default();
        let bytes = b"\x1bPqkitty-stuff\x1b\\rest";
        let scan = scanner.scan(bytes);
        let out = scan.forwarded.unwrap_or_else(|| bytes.to_vec());
        assert_eq!(out, bytes);
        assert!(scan.frames.is_empty());
    }

    #[test]
    fn a_prefix_that_diverges_across_a_chunk_boundary_is_flushed() {
        let mut scanner = OutputScanner::default();
        let first = scanner.scan(b"\x1bP+am"); // could still be ours: withheld
        assert_eq!(first.forwarded.as_deref(), Some(&b""[..]));
        let second = scanner.scan(b"ateur\x1b\\x"); // diverges: all of it comes back
        assert_eq!(second.forwarded.unwrap(), b"\x1bP+amateur\x1b\\x");
        assert!(second.frames.is_empty());
    }

    #[test]
    fn a_carried_candidate_that_keeps_matching_stays_withheld() {
        // Chunk boundaries inside the prefix must not leak the middle bytes.
        let mut scanner = OutputScanner::default();
        assert_eq!(scanner.scan(b"\x1bP").forwarded.as_deref(), Some(&b""[..]));
        assert_eq!(scanner.scan(b"+a").forwarded.as_deref(), Some(&b""[..]));
        let diverged = scanner.scan(b"Xrest");
        assert_eq!(diverged.forwarded.unwrap(), b"\x1bP+aXrest");
    }

    #[test]
    fn a_trailing_escape_is_withheld_and_flushed_next_chunk() {
        let mut scanner = OutputScanner::default();
        let first = scanner.scan(b"abc\x1b");
        assert_eq!(first.forwarded.unwrap(), b"abc");
        let second = scanner.scan(b"[0m");
        assert_eq!(second.forwarded.unwrap(), b"\x1b[0m");
    }

    #[test]
    fn an_unparseable_payload_is_dropped_without_reaching_the_terminal() {
        let mut scanner = OutputScanner::default();
        let scan = scanner.scan(b"a\x1bP+amon;1;not json\x1b\\b");
        assert_eq!(scan.forwarded.unwrap(), b"ab");
        assert!(scan.frames.is_empty(), "discarded, not surfaced");
    }

    #[test]
    fn an_oversized_payload_is_consumed_and_discarded() {
        let mut scanner = OutputScanner::default();
        let mut bytes = PREFIX.to_vec();
        bytes.extend(std::iter::repeat_n(b'x', 70 * 1024));
        bytes.extend_from_slice(ST);
        bytes.extend_from_slice(b"tail");
        let scan = scanner.scan(&bytes);
        assert_eq!(scan.forwarded.unwrap(), b"tail");
        assert!(scan.frames.is_empty());
    }

    #[test]
    fn two_whispers_in_one_chunk_both_come_out() {
        let mut scanner = OutputScanner::default();
        let mut bytes = encode(&WhisperFrame::Knock);
        bytes.extend_from_slice(&encode(&WhisperFrame::Bye));
        let scan = scanner.scan(&bytes);
        assert_eq!(scan.frames, vec![WhisperFrame::Knock, WhisperFrame::Bye]);
        assert_eq!(scan.forwarded.unwrap(), b"");
    }

    #[test]
    fn a_corrupt_whisper_is_swallowed_whole() {
        // An ESC inside a confirmed payload that does not begin ST: the
        // sequence was ours by prefix, so none of it may reach the terminal.
        let mut scanner = OutputScanner::default();
        let scan = scanner.scan(b"a\x1bP+amon;1;oops\x1bXb");
        assert_eq!(scan.forwarded.unwrap(), b"ab");
        assert!(scan.frames.is_empty());
    }
}
