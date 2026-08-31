use super::*;
use amon_detect::identify_agent;

fn claude() -> Agent {
    identify_agent("claude").expect("claude is a known agent")
}

fn input(screen: &str) -> DetectionInput<'_> {
    DetectionInput {
        screen,
        osc_title: "",
        osc_progress: "",
    }
}

/// A screen shaped like the ones the carriers are written against: a
/// transcript, then the input box drawn between two horizontal rules.
fn screen(transcript: &str) -> String {
    format!(
        "{transcript}\n\
         ────────────────────────────────────────\n\
         ❯ \n\
         ────────────────────────────────────────\n\
         ⏵⏵ auto mode on (shift+tab to cycle)\n"
    )
}

#[test]
fn every_carrier_names_an_agent_and_a_pattern_this_build_understands() {
    // The table ships in the binary, so a typo here is a bug that never
    // reaches a user — as long as something looks.
    let table: carriers::Table =
        toml::from_str(carriers::TABLE).expect("carriers.toml is valid TOML");
    assert!(!table.carrier.is_empty(), "the table would do nothing");
    for entry in &table.carrier {
        assert!(
            identify_agent(&entry.agent).is_some(),
            "carrier names unknown agent {:?}",
            entry.agent
        );
        assert!(
            regex::Regex::new(&entry.pattern).is_ok(),
            "carrier for {} has an uncompilable pattern",
            entry.agent
        );
    }
}

#[test]
fn every_carrier_names_a_region_the_engine_resolves() {
    // An unknown region name is not an error: the engine returns the empty
    // string for it, so a typo would silently mean "never any activity".
    let screen = screen("● something happened");
    let table: carriers::Table = toml::from_str(carriers::TABLE).expect("valid TOML");
    for entry in &table.carrier {
        assert!(
            !region(input(&screen), &entry.region).is_empty(),
            "carrier for {} names region {:?}, which resolved to nothing",
            entry.agent,
            entry.region
        );
    }
}

#[test]
fn the_marker_line_is_read_without_its_marker() {
    let screen = screen("● Read 5 files");
    assert_eq!(
        read(claude(), input(&screen)).as_deref(),
        Some("Read 5 files")
    );
}

#[test]
fn the_newest_marker_line_wins() {
    let screen = screen("● Read 5 files\n  ⎿ done\n● Bash(cargo test)");
    assert_eq!(
        read(claude(), input(&screen)).as_deref(),
        Some("Bash(cargo test)")
    );
}

#[test]
fn what_the_user_is_typing_is_never_read_back_to_them() {
    // The carrier's region stops at the input box, so a prompt that happens to
    // contain a marker — pasted output, a bullet list — cannot impersonate the
    // agent narrating itself.
    let screen = "● Read 5 files\n\
         ────────────────────────────────────────\n\
         ❯ here is what I want\n\
         ● and a line I pasted\n\
         ────────────────────────────────────────\n\
         ⏵⏵ auto mode on\n";
    assert_eq!(
        read(claude(), input(screen)).as_deref(),
        Some("Read 5 files")
    );
}

#[test]
fn a_screen_with_no_input_box_says_nothing() {
    // A pager or an editor has the terminal. Whatever is on it is not the
    // agent's transcript, even if a line happens to look like one.
    let bare = "● Read 5 files\n(END)\n";
    assert_eq!(read(claude(), input(bare)), None);
}

#[test]
fn a_screen_herdr_distrusts_says_nothing() {
    // claude.toml's `transcript_viewer` rule sets skip_state_update, and at
    // priority 1000 it outranks the input box rule — so the box is still
    // drawn, the transcript is still behind it, and the screen is still not
    // to be believed. That is the case the box check alone cannot catch.
    //
    // Only the *winning* rule contributes the flag, which is why the
    // fixture has to be one where the skip rule actually wins. Inheriting
    // that quirk is deliberate: activity goes quiet exactly when state does,
    // rather than on some second opinion of amon's own.
    let viewer = "● Read 5 files\n\
         ────────────────────────────────────────\n\
         ❯ \n\
         ────────────────────────────────────────\n\
         Showing detailed transcript · ctrl+o to toggle\n";
    assert!(
        amon_detect::detect::manifest::explain_with_input(claude(), input(viewer))
            .skip_state_update,
        "the fixture stopped tripping the rule it was written for"
    );
    assert_eq!(read(claude(), input(viewer)), None);
}

#[test]
fn a_turn_with_no_marker_yet_says_nothing() {
    let screen = screen("⏵⏵ thinking");
    assert_eq!(read(claude(), input(&screen)), None);
}

#[test]
fn runs_of_grid_padding_collapse() {
    let screen = screen("●   Read   5   files");
    assert_eq!(
        read(claude(), input(&screen)).as_deref(),
        Some("Read 5 files")
    );
}

#[test]
fn a_runaway_line_is_bounded() {
    let long = "x".repeat(500);
    let screen = screen(&format!("● {long}"));
    let read = read(claude(), input(&screen)).expect("a line");
    assert_eq!(read.chars().count(), MAX_CHARS + 1, "{read}");
    assert!(read.ends_with('…'));
}

#[test]
fn a_frame_that_cannot_be_read_holds_the_last_line() {
    // The marker blinks off while a tool call is in flight. Clearing on that
    // frame would make the column flicker between a line and nothing.
    let mut tracker = ActivityTracker::new();
    assert!(tracker.observe(claude(), input(&screen("● Bash(cargo test)"))));
    assert!(!tracker.observe(claude(), input(&screen("  ⎿ running"))));
    assert_eq!(tracker.current(), Some("Bash(cargo test)"));
}

#[test]
fn an_unchanged_line_is_not_a_change() {
    let mut tracker = ActivityTracker::new();
    let screen = screen("● Read 5 files");
    assert!(tracker.observe(claude(), input(&screen)));
    assert!(!tracker.observe(claude(), input(&screen)));
}

#[test]
fn a_finished_session_clears() {
    let mut tracker = ActivityTracker::new();
    tracker.observe(claude(), input(&screen("● Read 5 files")));
    assert!(tracker.clear());
    assert_eq!(tracker.current(), None);
    assert!(!tracker.clear());
}
