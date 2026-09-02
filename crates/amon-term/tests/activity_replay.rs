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
use amon_term::{ActivityKind, ActivityTracker, ShadowTerminal};

const COLS: u16 = 100;
const ROWS: u16 = 30;

/// Every Activity the reader produces over a capture, in order, with
/// consecutive repeats collapsed — the sequence a panel would have shown.
/// Prompts are rendered with the ❯ the panel gives them, so the transcript
/// below reads the way a row would.
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
            let current = tracker.current().expect("just set");
            seen.push(match current.kind {
                ActivityKind::Prompt => format!("\u{276F} {}", current.text),
                ActivityKind::Narration => current.text.clone(),
            });
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
        assert!(
            !line.starts_with(['●', '•', '■', '✗', '✓']),
            "marker was left on {line:?}"
        );
        assert!(!line.contains("esc to interrupt"), "status line: {line:?}");
        assert!(!line.contains("ctrl+o"), "kept a keyboard hint: {line:?}");
        assert!(
            !line.starts_with("Working"),
            "the label we exist to avoid: {line:?}"
        );
        assert!(line.len() <= 200, "unbounded: {line:?}");
    }
}

#[test]
fn a_session_that_lists_a_directory_reads_as_the_steps_it_took() {
    let seen = replay("claude-directory-listing.raw", "claude");
    assert_reads_as_the_agents_own_words(&seen);
    // The session opens on the ask, narrates the listing in the harness's
    // own words, and closes on the reply's first line. None of it is a
    // phrase amon composed.
    assert!(
        seen.iter()
            .any(|line| !line.starts_with('\u{276F}') && line.contains("directory")),
        "never narrated the listing: {seen:#?}"
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

    // A session now opens on the ask — the thinking gap, filled — and the
    // harness's own present tense takes over the moment it narrates. The
    // tense difference is the harness's; amon relays it rather than
    // flattening both to "Working".
    assert!(
        seen.first()
            .is_some_and(|line| line.starts_with('\u{276F}')),
        "opened on {:?}",
        seen.first()
    );
    assert!(
        seen.iter().any(|line| line.starts_with("Reading")),
        "never narrated in the present tense: {seen:#?}"
    );
    assert!(
        seen.last()
            .is_some_and(|line| line.contains("Apache License")),
        "closed on {:?}",
        seen.last()
    );
}

#[test]
fn the_prompt_appears_as_a_prompt_and_never_as_narration() {
    // The submitted ask is the Turn's opening Activity — marked as the
    // user's words — and must never leak into the sequence dressed as the
    // agent's own narration.
    let seen = replay("claude-directory-listing.raw", "claude");
    assert!(
        seen.iter()
            .any(|line| line.starts_with('\u{276F}') && line.contains("List the files in")),
        "the thinking gap showed no prompt: {seen:#?}"
    );
    for line in &seen {
        if line.contains("List the files in") {
            assert!(
                line.starts_with('\u{276F}'),
                "prompt dressed as narration: {line:?}"
            );
        }
    }
}

#[test]
fn a_codex_session_reads_as_the_steps_it_took() {
    // Codex narrates a step in the present tense while it runs and in the
    // past tense once it is done, then states the answer. The tense shift is
    // the harness's; amon relays both rather than flattening them.
    let seen = replay("codex-prompt.raw", "codex");
    assert_reads_as_the_agents_own_words(&seen);
    assert!(
        seen.iter().any(|line| line.starts_with("Explor")),
        "never saw the step it took: {seen:?}"
    );
    assert!(
        seen.last()
            .is_some_and(|line| line.contains("Apache License")),
        "closed on {:?}",
        seen.last()
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
