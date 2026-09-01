//! Replays captured agent sessions and checks what the activity reader makes
//! of them, frame by frame.
//!
//! A capture is a raw PTY stream, so it renders to a byte-identical screen
//! forever and CI can hold the reader to a real session without running an
//! agent. Feeding it in slices rather than all at once is the point: the
//! frames worth checking are the transient ones — the marker blinking off
//! mid-tool-call, a redraw caught half-finished — and a capture replayed in
//! one go shows only its final screen.

use amon_detect::detect::manifest::DetectionInput;
use amon_detect::identify_agent;
use amon_term::{read_activity, ActivityTracker, ShadowTerminal};

const COLS: u16 = 100;
const ROWS: u16 = 30;

/// Every activity line the reader produces over a capture, in order, with
/// consecutive repeats collapsed — the sequence a panel would have shown.
fn replay(name: &str, agent: &str) -> Vec<String> {
    let agent = identify_agent(agent).expect("known agent");
    let path = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
    let bytes = std::fs::read(&path).expect("fixture");

    let mut shadow = ShadowTerminal::new(COLS, ROWS).expect("shadow terminal");
    let mut tracker = ActivityTracker::new();
    let mut seen = Vec::new();
    for chunk in bytes.chunks(256) {
        shadow.feed(chunk);
        let screen = shadow.detection_text();
        let title = shadow.title().unwrap_or_default();
        let input = DetectionInput {
            screen: &screen,
            osc_title: &title,
            osc_progress: "",
        };
        if tracker.observe(agent, input) {
            seen.push(tracker.current().expect("just set").to_string());
        }
    }
    seen
}

/// Nothing amon composed, and nothing that belongs to the terminal rather
/// than to the agent. Applied to every captured session, since these are the
/// ways a screen reader goes wrong regardless of what was asked.
fn assert_reads_as_the_agents_own_words(seen: &[String]) {
    assert!(!seen.is_empty(), "read nothing from a whole session");
    for line in seen {
        assert!(!line.starts_with('●'), "marker was left on {line:?}");
        assert!(!line.contains("esc to interrupt"), "status line: {line:?}");
        assert!(!line.contains("ctrl+o"), "kept a keyboard hint: {line:?}");
        assert!(line.len() <= 200, "unbounded: {line:?}");
    }
}

#[test]
fn a_session_that_lists_a_directory_reads_as_the_steps_it_took() {
    let seen = replay("claude-directory-listing.raw", "claude");
    assert_reads_as_the_agents_own_words(&seen);
    // The prompt asked for a listing, so the session opens on the harness
    // narrating that in its own present tense and closes on the reply's first
    // line. Neither is a phrase amon composed.
    assert!(
        seen.first().is_some_and(|line| line.contains("directory")),
        "opened on {:?}",
        seen.first()
    );
    assert!(
        seen.last()
            .is_some_and(|line| line.contains("crates/amon-term/src")),
        "closed on {:?}",
        seen.last()
    );
}

#[test]
fn a_session_that_reads_a_file_narrates_it_in_the_present_tense() {
    // Captured through the wrapper against the real daemon, so this is the
    // screen amon actually sees in service rather than one a probe drove.
    let seen = replay("claude-file-read.raw", "claude");
    assert_reads_as_the_agents_own_words(&seen);

    // The harness says "Reading 1 file…" while it works and states the answer
    // when it is done. Both are its own phrasing — the tense difference is the
    // harness's, and amon relays it rather than flattening both to "Working".
    assert!(
        seen.first().is_some_and(|line| line.starts_with("Reading")),
        "opened on {:?}",
        seen.first()
    );
    assert!(
        seen.last()
            .is_some_and(|line| line.contains("Apache License")),
        "closed on {:?}",
        seen.last()
    );
}

#[test]
fn a_claude_session_never_reads_the_user_own_prompt() {
    // The prompt the probe typed is echoed into the input box and stays on
    // screen for the whole session. Reading it back would be the column
    // telling the user what they just said.
    for line in replay("claude-directory-listing.raw", "claude") {
        assert!(
            !line.contains("List the files in"),
            "read the user's prompt: {line:?}"
        );
    }
}

#[test]
fn an_agent_with_no_carrier_reads_nothing() {
    // Only claude has an entry today. The reader has to be silent for the
    // rest rather than guess, since every other agent's screen is still
    // uncharacterised.
    let agent = identify_agent("codex").expect("known agent");
    let path = format!(
        "{}/tests/fixtures/codex-prompt.raw",
        env!("CARGO_MANIFEST_DIR")
    );
    let bytes = std::fs::read(&path).expect("fixture");
    let mut shadow = ShadowTerminal::new(COLS, ROWS).expect("shadow terminal");
    shadow.feed(&bytes);
    let screen = shadow.detection_text();
    assert_eq!(
        read_activity(
            agent,
            DetectionInput {
                screen: &screen,
                osc_title: "",
                osc_progress: "",
            }
        ),
        None
    );
}

#[test]
#[ignore = "prints the sequence a capture produces, for eyeballing a new fixture"]
fn show() {
    let fixture = std::env::var("FIXTURE").unwrap_or("claude-directory-listing.raw".into());
    let agent = std::env::var("AGENT").unwrap_or("claude".into());
    for (index, line) in replay(&fixture, &agent).iter().enumerate() {
        println!("{index:3}  {line}");
    }
}
