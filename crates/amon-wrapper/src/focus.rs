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
//! missed focus event on the rare split; the alternative is a sluggish agent.

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

/// What the observer knows and the input thread needs.
///
/// The mode lives in the shadow terminal, which the observer owns, but the
/// decision to forward or swallow is made on the input thread. Two flags are
/// cheaper than a channel and cannot block either side.
#[derive(Clone, Default)]
pub struct Shared {
    /// The agent enabled focus reporting itself, so the reports are its own
    /// and must reach it untouched.
    pub agent_wants: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// The agent turned focus reporting off, taking the wrapper's own
    /// reporting with it. Set here, acted on where writing to the terminal is
    /// safe.
    pub reassert: std::sync::Arc<std::sync::atomic::AtomicBool>,
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
