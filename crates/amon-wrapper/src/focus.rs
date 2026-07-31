//! Where the user is looking.
//!
//! Terminals report focus as `CSI I` (gained) and `CSI O` (lost) on the input
//! stream once DEC mode 1004 is on. The wrapper turns that mode on itself
//! rather than hoping the agent did (ADR-0007), which is why the sequences
//! have to be taken back out of the input again: bytes the agent never asked
//! for must not reach it.
//!
//! Nothing here delays a keystroke. A sequence split across two reads is
//! forwarded rather than held, because holding an `ESC` to see what follows
//! would add latency to the single most-pressed key in a TUI. The cost is a
//! missed focus event on the rare split, and a literal `CSI I` typed or pasted
//! by the user being taken for a focus report; the alternative is the escape
//! latency multiplexers are notorious for. See ADR-0007.

/// Asks the terminal to report focus changes.
pub const ENABLE: &[u8] = b"\x1b[?1004h";
/// Stops focus reporting. What the agent's terminal looked like before amon.
pub const DISABLE: &[u8] = b"\x1b[?1004l";

/// What a read from the user's keyboard turned out to contain.
pub struct Scan {
    /// Focus transitions in the order they appeared: `true` gained, `false`
    /// lost.
    pub events: Vec<bool>,
    /// The input with the focus sequences removed — `None` when there were
    /// none, so the untouched buffer can be forwarded as-is.
    pub stripped: Option<Vec<u8>>,
}

/// Finds focus reports in a chunk of input.
pub fn scan(input: &[u8]) -> Scan {
    let mut events = Vec::new();
    let mut stripped: Option<Vec<u8>> = None;
    let mut copied = 0;
    let mut index = 0;

    while index + 3 <= input.len() {
        let gained = match &input[index..index + 3] {
            [0x1b, b'[', b'I'] => true,
            [0x1b, b'[', b'O'] => false,
            _ => {
                index += 1;
                continue;
            }
        };
        let out = stripped.get_or_insert_with(|| Vec::with_capacity(input.len()));
        out.extend_from_slice(&input[copied..index]);
        events.push(gained);
        index += 3;
        copied = index;
    }

    if let Some(out) = stripped.as_mut() {
        out.extend_from_slice(&input[copied..]);
    }
    Scan { events, stripped }
}

/// What the output path knows and the input thread needs.
///
/// Read on every keystroke, so these are atomics rather than anything the
/// output path could make an input thread wait behind.
#[derive(Clone, Default)]
pub struct Shared {
    /// The agent enabled focus reporting itself, so the reports are its own
    /// and must reach it untouched.
    pub agent_wants: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// The agent turned focus reporting off, taking the wrapper's own
    /// reporting with it. Set on the output path, acted on where writing to
    /// the terminal is safe.
    pub reassert: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

/// Follows the agent's focus-reporting mode along the output stream.
///
/// This has to live on the output path rather than in the shadow terminal:
/// what it decides governs the agent's *input*, and the observer is
/// best-effort — a shadow terminal that failed to start, or fell behind, must
/// never change what the agent receives. It is also what says whether the
/// stream is between escape sequences, which is the only safe moment for amon
/// to write anything of its own.
#[derive(Default)]
pub struct ModeScanner {
    state: State,
    /// Parameters of the CSI being read, newest last.
    params: Vec<u32>,
    private: bool,
    agent_wants: bool,
}

#[derive(Default, PartialEq, Eq, Clone, Copy)]
enum State {
    #[default]
    Ground,
    Escape,
    Csi,
    /// Inside an OSC/DCS/APC string, which ends at BEL or ST.
    String_,
    /// Inside such a string, having just seen the ESC of a possible ST.
    StringEscape,
}

/// What changed while a chunk of output went past.
#[derive(Default, PartialEq, Eq, Debug)]
pub struct ModeChange {
    /// The agent's focus-reporting mode after this chunk.
    pub agent_wants: bool,
    /// The agent turned the mode off at some point in this chunk — including
    /// turning it off when it was already off, which still reaches the real
    /// terminal and still takes amon's own reporting with it.
    pub disabled: bool,
}

impl ModeScanner {
    /// Feeds a chunk of the agent's output. Never call this with bytes the
    /// terminal did not also receive.
    pub fn feed(&mut self, bytes: &[u8]) -> ModeChange {
        let mut change = ModeChange {
            agent_wants: self.agent_wants,
            disabled: false,
        };
        for byte in bytes {
            self.step(*byte, &mut change);
        }
        change.agent_wants = self.agent_wants;
        change
    }

    /// Whether the stream is between escape sequences. Injecting anything of
    /// amon's own at any other moment would land inside the agent's sequence
    /// and change what it means.
    pub fn at_boundary(&self) -> bool {
        self.state == State::Ground
    }

    fn step(&mut self, byte: u8, change: &mut ModeChange) {
        const MAX_PARAMS: usize = 32;
        match self.state {
            State::Ground => {
                if byte == 0x1b {
                    self.state = State::Escape;
                }
            }
            State::Escape => match byte {
                b'[' => {
                    self.state = State::Csi;
                    self.params.clear();
                    self.private = false;
                }
                b']' | b'P' | b'X' | b'^' | b'_' => self.state = State::String_,
                // RIS: every mode goes back to its default, amon's included.
                b'c' => {
                    change.disabled = true;
                    self.agent_wants = false;
                    self.state = State::Ground;
                }
                _ => self.state = State::Ground,
            },
            State::Csi => match byte {
                b'?' if self.params.is_empty() => self.private = true,
                b'0'..=b'9' => {
                    if self.params.is_empty() {
                        self.params.push(0);
                    }
                    if let Some(last) = self.params.last_mut() {
                        *last = last
                            .saturating_mul(10)
                            .saturating_add(u32::from(byte - b'0'));
                    }
                }
                b';' if self.params.len() < MAX_PARAMS => self.params.push(0),
                b';' => {}
                // Intermediates and anything else in the middle: keep reading.
                0x20..=0x2f => {}
                0x40..=0x7e => {
                    let mentions_focus = self.params.contains(&1004);
                    if self.private && mentions_focus {
                        match byte {
                            b'h' => self.agent_wants = true,
                            b'l' => {
                                change.disabled = true;
                                self.agent_wants = false;
                            }
                            _ => {}
                        }
                    }
                    self.state = State::Ground;
                }
                _ => {}
            },
            State::String_ => match byte {
                0x07 => self.state = State::Ground,
                0x1b => self.state = State::StringEscape,
                _ => {}
            },
            State::StringEscape => {
                // ESC \ ends the string; anything else was a stray ESC.
                self.state = if byte == b'\\' {
                    State::Ground
                } else {
                    State::String_
                };
            }
        }
    }
}

/// Focus and Seen for one agent.
///
/// Seen is "has the user looked at this since the agent last changed state",
/// which is what makes an unnoticed `Idle` tellable from one the user watched
/// finish. It resets on every state change and only ever moves to `true`
/// within a state — looking away does not unsee.
#[derive(Default)]
pub struct Tracker {
    focused: Option<bool>,
    seen: Option<bool>,
}

impl Tracker {
    /// Records a focus transition. Returns whether anything changed.
    pub fn focus_changed(&mut self, focused: bool) -> bool {
        let before = (self.focused, self.seen);
        self.focused = Some(focused);
        if focused {
            self.seen = Some(true);
        }
        before != (self.focused, self.seen)
    }

    /// Records that the agent entered a new state, restarting Seen. Focus at
    /// this instant counts as having seen it: the user is already looking.
    pub fn state_began(&mut self) {
        self.seen = self.focused;
    }

    pub fn focused(&self) -> Option<bool> {
        self.focused
    }

    pub fn seen(&self) -> Option<bool> {
        self.seen
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn input_without_focus_reports_is_left_alone() {
        let scan = scan(b"ls -la\r");
        assert!(scan.events.is_empty());
        assert!(
            scan.stripped.is_none(),
            "an untouched buffer is forwarded as-is"
        );
    }

    #[test]
    fn focus_reports_are_taken_out_of_the_input() {
        let scan = scan(b"\x1b[Ihello\x1b[O!");
        assert_eq!(scan.events, vec![true, false]);
        assert_eq!(scan.stripped.unwrap(), b"hello!");
    }

    #[test]
    fn a_report_split_across_reads_is_forwarded_rather_than_held() {
        // Holding the trailing ESC would delay every ESC keypress.
        let first = scan(b"abc\x1b[");
        assert!(first.events.is_empty());
        assert!(first.stripped.is_none());

        let second = scan(b"I");
        assert!(second.events.is_empty());
        assert!(second.stripped.is_none());
    }

    #[test]
    fn an_escape_that_is_not_a_focus_report_survives() {
        let scan = scan(b"\x1b[A\x1b[3~");
        assert!(scan.events.is_empty());
        assert!(scan.stripped.is_none());
    }

    #[test]
    fn back_to_back_reports_are_all_found() {
        let scan = scan(b"\x1b[O\x1b[I");
        assert_eq!(scan.events, vec![false, true]);
        assert_eq!(scan.stripped.unwrap(), b"");
    }

    #[test]
    fn an_agent_that_says_nothing_wants_no_focus_reports() {
        let mut scanner = ModeScanner::default();
        let change = scanner.feed(b"hello \x1b[1mworld\x1b[0m\n");
        assert_eq!(
            change,
            ModeChange {
                agent_wants: false,
                disabled: false
            }
        );
        assert!(scanner.at_boundary());
    }

    #[test]
    fn enabling_and_disabling_focus_reporting_is_followed() {
        let mut scanner = ModeScanner::default();
        assert!(scanner.feed(b"\x1b[?1004h").agent_wants);
        let off = scanner.feed(b"\x1b[?1004l");
        assert!(!off.agent_wants);
        assert!(off.disabled, "the real terminal was turned off too");
    }

    #[test]
    fn a_disable_counts_even_when_the_agent_never_enabled_it() {
        // The agent's own state did not change, but the byte still reached the
        // terminal and turned amon's reporting off with it — the case a
        // before/after comparison of the mode misses entirely.
        let mut scanner = ModeScanner::default();
        let change = scanner.feed(b"\x1b[?1004l");
        assert!(!change.agent_wants);
        assert!(change.disabled);
    }

    #[test]
    fn a_reset_takes_focus_reporting_with_it() {
        let mut scanner = ModeScanner::default();
        scanner.feed(b"\x1b[?1004h");
        let change = scanner.feed(b"\x1bc");
        assert!(!change.agent_wants, "RIS restores mode defaults");
        assert!(change.disabled);
    }

    #[test]
    fn a_mode_change_split_across_chunks_is_still_seen() {
        // Chunk boundaries are wherever the pty happened to split; they are
        // not sequence boundaries.
        let mut scanner = ModeScanner::default();
        assert!(!scanner.feed(b"\x1b[?10").agent_wants);
        assert!(!scanner.at_boundary(), "mid-sequence");
        assert!(scanner.feed(b"04h").agent_wants);
        assert!(scanner.at_boundary());
    }

    #[test]
    fn focus_is_found_among_other_modes() {
        let mut scanner = ModeScanner::default();
        assert!(scanner.feed(b"\x1b[?1000;1004;2004h").agent_wants);
        assert!(!scanner.feed(b"\x1b[?1000;1004;2004l").agent_wants);
    }

    #[test]
    fn a_mode_amon_does_not_care_about_changes_nothing() {
        let mut scanner = ModeScanner::default();
        scanner.feed(b"\x1b[?1004h");
        let change = scanner.feed(b"\x1b[?25l\x1b[?2004h");
        assert!(change.agent_wants, "still on");
        assert!(!change.disabled);
    }

    #[test]
    fn an_ansi_mode_is_not_a_private_one() {
        // `CSI 1004 h` without the `?` is a different namespace.
        let mut scanner = ModeScanner::default();
        assert!(!scanner.feed(b"\x1b[1004h").agent_wants);
    }

    #[test]
    fn a_mode_sequence_inside_a_title_is_only_text() {
        // OSC strings carry arbitrary bytes; nothing in them is a command.
        let mut scanner = ModeScanner::default();
        let change = scanner.feed(b"\x1b]0;\x1b[?1004h\x07");
        assert!(!change.agent_wants);
        assert!(scanner.at_boundary(), "the string terminated");
    }

    #[test]
    fn an_unterminated_sequence_is_never_a_safe_moment_to_write() {
        let mut scanner = ModeScanner::default();
        scanner.feed(b"\x1b[");
        assert!(!scanner.at_boundary());
        scanner.feed(b"0m");
        assert!(scanner.at_boundary());

        scanner.feed(b"\x1b]0;a title");
        assert!(!scanner.at_boundary(), "an OSC string is still open");
        scanner.feed(b"\x1b\\");
        assert!(scanner.at_boundary());
    }

    #[test]
    fn focus_is_unknown_until_something_says_otherwise() {
        let tracker = Tracker::default();
        assert_eq!(tracker.focused(), None);
        assert_eq!(tracker.seen(), None);
    }

    #[test]
    fn gaining_focus_marks_the_current_state_seen() {
        let mut tracker = Tracker::default();
        assert!(tracker.focus_changed(true));
        assert_eq!(tracker.focused(), Some(true));
        assert_eq!(tracker.seen(), Some(true));
    }

    #[test]
    fn looking_away_does_not_unsee() {
        let mut tracker = Tracker::default();
        tracker.focus_changed(true);
        tracker.focus_changed(false);
        assert_eq!(tracker.focused(), Some(false));
        assert_eq!(tracker.seen(), Some(true), "it was seen during this state");
    }

    #[test]
    fn a_new_state_starts_unseen_when_the_user_is_elsewhere() {
        let mut tracker = Tracker::default();
        tracker.focus_changed(true);
        tracker.focus_changed(false);

        tracker.state_began();

        assert_eq!(tracker.seen(), Some(false), "finished while looking away");
    }

    #[test]
    fn a_new_state_is_seen_immediately_when_the_user_is_watching() {
        let mut tracker = Tracker::default();
        tracker.focus_changed(true);

        tracker.state_began();

        assert_eq!(tracker.seen(), Some(true));
    }

    #[test]
    fn seen_stays_unknown_while_focus_is() {
        let mut tracker = Tracker::default();
        tracker.state_began();
        assert_eq!(tracker.seen(), None);
    }

    #[test]
    fn repeating_a_focus_state_is_not_a_change() {
        let mut tracker = Tracker::default();
        assert!(tracker.focus_changed(true));
        assert!(!tracker.focus_changed(true), "nothing to report twice");
    }
}
