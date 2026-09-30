use super::*;
use amon_detect::identify_agent;

fn claude() -> Agent {
    identify_agent("claude").expect("claude is a known agent")
}

fn codex() -> Agent {
    identify_agent("codex").expect("codex is a known agent")
}

fn input(screen: &str) -> DetectionInput<'_> {
    DetectionInput {
        screen,
        osc_title: "",
        osc_progress: "",
    }
}

/// A claude screen: a transcript, then the input box between two rules.
fn screen(transcript: &str) -> String {
    format!(
        "{transcript}\n\
         ────────────────────────────────────────\n\
         ❯ \n\
         ────────────────────────────────────────\n\
         ⏵⏵ auto mode on (shift+tab to cycle)\n"
    )
}

/// A codex screen: bullets for steps, a bare marker line for the live prompt,
/// no box drawn around anything.
fn codex_screen(transcript: &str) -> String {
    format!(
        "{transcript}\n\
         › Ask Codex to do anything\n\
         gpt-5.6-sol high · ~/somewhere\n"
    )
}

fn narration(text: &str) -> Option<Activity> {
    Some(Activity {
        text: text.to_string(),
        kind: ActivityKind::Narration,
    })
}

fn prompt(text: &str) -> Option<Activity> {
    Some(Activity {
        text: text.to_string(),
        kind: ActivityKind::Prompt,
    })
}

// ---- the table --------------------------------------------------------

#[test]
fn every_carrier_names_an_agent_and_patterns_this_build_understands() {
    let table: carriers::Table =
        toml::from_str(carriers::TABLE).expect("carriers.toml is valid TOML");
    assert!(!table.carrier.is_empty(), "the table would do nothing");
    for entry in &table.carrier {
        assert!(
            identify_agent(&entry.agent).is_some(),
            "carrier names unknown agent {:?}",
            entry.agent
        );
        for (field, pattern) in [
            ("pattern", Some(entry.pattern.as_str())),
            ("gate_pattern", entry.gate_pattern.as_deref()),
            ("reject", entry.reject.as_deref()),
            ("prompt_pattern", entry.prompt_pattern.as_deref()),
            ("prompt_reject", entry.prompt_reject.as_deref()),
        ] {
            let Some(pattern) = pattern else { continue };
            assert!(
                regex::Regex::new(pattern).is_ok(),
                "carrier for {} has an uncompilable {field}",
                entry.agent
            );
        }
        assert_eq!(
            entry.prompt_region.is_some(),
            entry.prompt_pattern.is_some(),
            "carrier for {} has half a prompt definition",
            entry.agent
        );
    }
}

#[test]
fn every_carrier_names_regions_the_engine_resolves() {
    // An unknown region name is not an error: the engine returns the empty
    // string for it, so a typo would silently mean "never any activity".
    let screen = screen("● something happened");
    let table: carriers::Table = toml::from_str(carriers::TABLE).expect("valid TOML");
    for entry in &table.carrier {
        let mut specs = vec![
            ("region", entry.region.clone()),
            ("gate_region", entry.gate_region.clone()),
        ];
        if let Some(spec) = &entry.prompt_region {
            specs.push(("prompt_region", spec.clone()));
        }
        for (field, spec) in specs {
            assert!(
                !region(input(&screen), &spec).is_empty(),
                "carrier for {} names {field} {spec:?}, which resolved to nothing",
                entry.agent
            );
        }
    }
}

// ---- narration --------------------------------------------------------

#[test]
fn the_marker_line_is_read_without_its_marker() {
    let screen = screen("● Read 5 files");
    assert_eq!(read(claude(), input(&screen)), narration("Read 5 files"));
}

#[test]
fn the_newest_marker_line_wins() {
    let screen = screen("● Read 5 files\n  ⎿ done\n● Bash(cargo test)");
    assert_eq!(
        read(claude(), input(&screen)),
        narration("Bash(cargo test)")
    );
}

#[test]
fn what_the_user_is_typing_is_never_read_back_to_them() {
    // Both regions stop at the input box, so a draft that happens to contain
    // a marker — pasted output, a bullet list — cannot impersonate either
    // kind of Activity.
    let screen = "● Read 5 files\n\
         ────────────────────────────────────────\n\
         ❯ here is what I want\n\
         ● and a line I pasted\n\
         ────────────────────────────────────────\n\
         ⏵⏵ auto mode on\n";
    assert_eq!(read(claude(), input(screen)), narration("Read 5 files"));
}

#[test]
fn a_screen_with_no_input_box_says_nothing() {
    let bare = "● Read 5 files\n(END)\n";
    assert_eq!(read(claude(), input(bare)), None);
}

#[test]
fn a_screen_herdr_distrusts_says_nothing() {
    // claude.toml's `transcript_viewer` rule sets skip_state_update, and at
    // priority 1000 it outranks the input box rule — so the box is still
    // drawn, the transcript is still behind it, and the screen is still not
    // to be believed. That is the case the gate alone cannot catch.
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
fn runs_of_grid_padding_collapse() {
    let screen = screen("●   Read   5   files");
    assert_eq!(read(claude(), input(&screen)), narration("Read 5 files"));
}

#[test]
fn a_runaway_line_is_bounded() {
    let long = "x".repeat(500);
    let screen = screen(&format!("● {long}"));
    let read = read(claude(), input(&screen)).expect("a line");
    assert_eq!(read.text.chars().count(), MAX_CHARS + 1, "{}", read.text);
    assert!(read.text.ends_with('…'));
}

// ---- the Turn rule ----------------------------------------------------

#[test]
fn a_turn_with_no_narration_yet_shows_its_prompt() {
    // The thinking gap: submitted, nothing narrated. The ask is the only
    // honest answer.
    let screen = screen("❯ refactor the auth module");
    assert_eq!(
        read(claude(), input(&screen)),
        prompt("refactor the auth module")
    );
}

#[test]
fn narration_after_the_prompt_wins_over_it() {
    let screen = screen("❯ refactor the auth module\n● Reading 3 files…");
    assert_eq!(
        read(claude(), input(&screen)),
        narration("Reading 3 files…")
    );
}

#[test]
fn narration_before_the_prompt_is_a_finished_turns_and_loses() {
    // The stale-turn failure: the newest marker line narrates the *previous*
    // question. The Turn rule is what keeps it off the row.
    let screen = screen(
        "● Apache License, Version 2.0\n\
         ❯ now delete everything and start over",
    );
    assert_eq!(
        read(claude(), input(&screen)),
        prompt("now delete everything and start over")
    );
}

#[test]
fn a_startup_tip_is_not_an_ask() {
    let screen = screen("❯ Try \"fix the failing test\"");
    assert_eq!(read(claude(), input(&screen)), None);
}

#[test]
fn a_slash_command_is_not_an_ask() {
    // "/rc" is an instruction to the harness, not something the agent will
    // narrate against; a Turn opened by it would never close.
    let screen = screen("❯ /rc");
    assert_eq!(read(claude(), input(&screen)), None);
}

#[test]
fn codex_orders_prompt_and_narration_across_different_regions() {
    // Codex's narration and prompt come from different regions of the same
    // screen; the whole-screen offset is what lets the Turn rule order them.
    let working = codex_screen("› read NOTICE and name the licence\n• Explored");
    assert_eq!(read(codex(), input(&working)), narration("Explored"));

    let thinking = codex_screen("• Explored\n› and now write it to a file");
    assert_eq!(
        read(codex(), input(&thinking)),
        prompt("and now write it to a file")
    );
}

#[test]
fn codexs_live_placeholder_is_not_an_ask() {
    // "› Ask Codex to do anything" is the live input line;
    // before_current_prompt_marker cuts it away structurally.
    let idle = codex_screen("");
    assert_eq!(read(codex(), input(&idle)), None);
}

// ---- the tracker ------------------------------------------------------

#[test]
fn a_frame_that_cannot_be_read_holds_the_last_activity() {
    let mut tracker = ActivityTracker::new();
    assert!(tracker.observe(claude(), input(&screen("● Bash(cargo test)"))));
    assert!(!tracker.observe(claude(), input(&screen("  ⎿ running"))));
    assert_eq!(tracker.current(), narration("Bash(cargo test)").as_ref());
}

#[test]
fn a_blinked_off_marker_does_not_regress_a_turn_to_its_ask() {
    // The flap the probe caught: prompt → narration → prompt → narration as
    // the marker blinks. Once this Turn has narrated, a frame showing only
    // its prompt is the blink, not news.
    let mut tracker = ActivityTracker::new();
    tracker.observe(claude(), input(&screen("❯ read the notice")));
    tracker.observe(
        claude(),
        input(&screen("❯ read the notice\n● Reading 1 file…")),
    );
    assert!(!tracker.observe(claude(), input(&screen("❯ read the notice"))));
    assert_eq!(tracker.current(), narration("Reading 1 file…").as_ref());
}

#[test]
fn a_new_prompt_replaces_the_previous_turn() {
    let mut tracker = ActivityTracker::new();
    tracker.observe(
        claude(),
        input(&screen("❯ read the notice\n● Apache License, 2.0")),
    );
    assert!(tracker.observe(
        claude(),
        input(&screen("● Apache License, 2.0\n❯ now summarise it"))
    ));
    assert_eq!(tracker.current(), prompt("now summarise it").as_ref());
}

#[test]
fn a_hook_opens_a_turn_with_the_exact_text() {
    let mut tracker = ActivityTracker::new();
    assert!(tracker.begin_turn("refactor the auth module\nwith tests please"));
    assert_eq!(
        tracker.current(),
        prompt("refactor the auth module").as_ref()
    );
}

#[test]
fn the_screens_rendering_of_a_hook_prompt_is_the_same_turn() {
    // The hook carries the prompt as typed; the screen wraps it to the
    // terminal, so its first line is a prefix. That is the same Turn, and
    // the exact text stays.
    let mut tracker = ActivityTracker::new();
    tracker.begin_turn("refactor the auth module and make the tests pass again");
    assert!(!tracker.observe(
        claude(),
        input(&screen("❯ refactor the auth module and make the"))
    ));
    assert_eq!(
        tracker.current(),
        prompt("refactor the auth module and make the tests pass again").as_ref()
    );
}

#[test]
fn narration_still_beats_a_hook_opened_turns_prompt() {
    // Kind first, then source: the doing outranks the ask wherever the ask
    // came from.
    let mut tracker = ActivityTracker::new();
    tracker.begin_turn("read the notice");
    assert!(tracker.observe(
        claude(),
        input(&screen("❯ read the notice\n● Reading 1 file…"))
    ));
    assert_eq!(tracker.current(), narration("Reading 1 file…").as_ref());
}

#[test]
fn a_finished_session_clears() {
    let mut tracker = ActivityTracker::new();
    tracker.observe(claude(), input(&screen("● Read 5 files")));
    assert!(tracker.clear());
    assert_eq!(tracker.current(), None);
    assert!(!tracker.clear());
}

#[test]
fn an_agent_with_no_carrier_reads_nothing() {
    // pi has no entry. Its screen tells prompts, tool calls and replies apart
    // by background colour alone, which no text region can express — so the
    // reader stays silent rather than reading a line back that might be the
    // user's own.
    let pi = identify_agent("pi").expect("pi is a known agent");
    assert_eq!(read(pi, input(&screen("● Read 5 files"))), None);
}

// ---- turn-boundary regressions (codex review) -------------------------

#[test]
fn a_new_prompt_and_its_narration_in_one_frame_do_not_regress_later() {
    // The frame where a new prompt appears together with its first narration
    // (the prompt-only frame was missed) must still record the new Turn, so a
    // later prompt-only frame reads as the marker blinking, not a regression.
    let mut tracker = ActivityTracker::new();
    tracker.observe(
        claude(),
        input(&screen("❯ the first ask\n● narrating the first")),
    );
    // New prompt and its narration arrive together.
    assert!(tracker.observe(
        claude(),
        input(&screen(
            "● narrating the first\n❯ the second ask\n● Reading 2 files…"
        ))
    ));
    assert_eq!(tracker.current(), narration("Reading 2 files…").as_ref());
    // Marker blinks off: only the second ask is on screen now.
    assert!(!tracker.observe(claude(), input(&screen("❯ the second ask"))));
    assert_eq!(
        tracker.current(),
        narration("Reading 2 files…").as_ref(),
        "regressed to the ask when the marker blinked"
    );
}

#[test]
fn a_hook_turn_is_not_clobbered_by_the_previous_turns_narration() {
    // A hook opens a Turn before the terminal redraws; the screen still shows
    // the previous turn's prompt and narration. That stale narration must not
    // replace the authoritative hook prompt.
    let mut tracker = ActivityTracker::new();
    // A previous turn was on screen when the hook fired (the realistic case).
    tracker.observe(
        claude(),
        input(&screen("❯ read the notice\n● Apache License, 2.0")),
    );
    tracker.begin_turn("summarise the findings");
    // Screen is still on the previous turn.
    assert!(!tracker.observe(
        claude(),
        input(&screen("❯ read the notice\n● Apache License, 2.0"))
    ));
    assert_eq!(
        tracker.current(),
        prompt("summarise the findings").as_ref(),
        "the previous turn's narration clobbered the hook prompt"
    );
    // Screen catches up (wrapped), now narrating the new turn.
    assert!(tracker.observe(
        claude(),
        input(&screen("❯ summarise the\n● Writing summary…"))
    ));
    assert_eq!(tracker.current(), narration("Writing summary…").as_ref());
}

#[test]
fn a_hook_turns_exact_text_survives_the_screen_echo() {
    // Once the screen echoes the hook's prompt (wrapped to width), the row
    // keeps the hook's exact text, not the truncated on-screen rendering.
    let mut tracker = ActivityTracker::new();
    tracker.begin_turn("refactor the auth module and keep the tests green");
    assert!(!tracker.observe(
        claude(),
        input(&screen("❯ refactor the auth module and keep the"))
    ));
    assert_eq!(
        tracker.current(),
        prompt("refactor the auth module and keep the tests green").as_ref()
    );
}

#[test]
fn a_hook_prompt_extending_the_previous_is_not_mistaken_for_it() {
    // codex round 2: the old prompt is a prefix of the new hook prompt. The
    // hold must key on the previous prompt exactly, so the stale narration of
    // "Fix bug" cannot be emitted for the "Fix bug thoroughly" turn.
    let mut tracker = ActivityTracker::new();
    tracker.observe(claude(), input(&screen("❯ Fix bug\n● patching one file")));
    tracker.begin_turn("Fix bug thoroughly");
    // Screen still on the previous turn, whose prompt is a prefix of the new.
    assert!(!tracker.observe(claude(), input(&screen("❯ Fix bug\n● patching one file"))));
    assert_eq!(
        tracker.current(),
        prompt("Fix bug thoroughly").as_ref(),
        "released the hold on a prefix collision and showed stale narration"
    );
    // Screen catches up to the new turn.
    assert!(tracker.observe(
        claude(),
        input(&screen("❯ Fix bug thoroughly\n● Reading 4 files…"))
    ));
    assert_eq!(tracker.current(), narration("Reading 4 files…").as_ref());
}

#[test]
fn re_submitting_the_same_prompt_holds_the_ask_then_releases() {
    // Re-submitting an identical prompt: the previous turn's narration is
    // still on screen and no prompt text can tell the turns apart, so the row
    // shows the ask through the thinking phase (not the stale narration), then
    // the cap lets narration take over rather than holding forever.
    let mut tracker = ActivityTracker::new();
    tracker.observe(
        claude(),
        input(&screen("❯ run the tests\n● Bash(cargo test)")),
    );
    tracker.begin_turn("run the tests");
    // First frame after re-submit still shows the old narration — held.
    assert!(!tracker.observe(
        claude(),
        input(&screen("❯ run the tests\n● Bash(cargo test)"))
    ));
    assert_eq!(
        tracker.current(),
        prompt("run the tests").as_ref(),
        "showed stale narration during the new turn's thinking phase"
    );
    let mut released = false;
    for _ in 0..(HOLD_FRAMES as usize + 1) {
        tracker.observe(
            claude(),
            input(&screen("❯ run the tests\n● Bash(cargo test)")),
        );
        if tracker.current().map(|a| a.kind) == Some(ActivityKind::Narration) {
            released = true;
            break;
        }
    }
    assert!(
        released,
        "the hold never released for an identical re-submit"
    );
}

#[test]
fn hook_origin_does_not_leak_prefix_matching_into_the_next_turn() {
    // codex round 3: a hook turn narrates, then a new turn's prompt is a
    // prefix of the previous boundary. Prefix matching must be off by then, or
    // the new ask is mistaken for the old turn and its narration lingers.
    let mut tracker = ActivityTracker::new();
    tracker.begin_turn("deploy to prod now");
    tracker.observe(
        claude(),
        input(&screen("❯ deploy to prod now\n● Deploying…")),
    );
    assert_eq!(tracker.current(), narration("Deploying…").as_ref());
    // New turn, its prompt a prefix of the previous ("deploy to prod now").
    assert!(tracker.observe(claude(), input(&screen("● Deploying…\n❯ deploy to prod"))));
    assert_eq!(
        tracker.current(),
        prompt("deploy to prod").as_ref(),
        "prefix matching leaked and swallowed the new turn"
    );
}

#[test]
fn a_long_previous_hook_prompt_shown_wrapped_still_holds() {
    // codex round 4: turn 1 is a long hook prompt the screen wraps; turn 2 is
    // a different hook prompt submitted before turn 1 narrated. The stale
    // frame still shows turn 1 wrapped — a prefix of its exact text, which the
    // hold must recognise so it does not clobber turn 2's ask.
    let long = "investigate the flaky integration test and report the root cause";
    let mut tracker = ActivityTracker::new();
    tracker.begin_turn(long); // turn 1
                              // Screen echoes turn 1 wrapped; establishes it as the on-screen turn.
    tracker.observe(
        claude(),
        input(&screen("❯ investigate the flaky integration test and")),
    );
    assert_eq!(tracker.current(), prompt(long).as_ref());

    tracker.begin_turn("now revert it"); // turn 2, before turn 1 narrated
                                         // Stale frame: screen still on turn 1, wrapped (a prefix of the long text).
    assert!(!tracker.observe(
        claude(),
        input(&screen("❯ investigate the flaky integration test and"))
    ));
    assert_eq!(
        tracker.current(),
        prompt("now revert it").as_ref(),
        "the wrapped previous prompt clobbered the new ask"
    );
    // Screen catches up to turn 2.
    assert!(tracker.observe(claude(), input(&screen("❯ now revert it\n● Reverting…"))));
    assert_eq!(tracker.current(), narration("Reverting…").as_ref());
}

#[test]
fn the_hold_is_bounded_when_prompts_are_indistinguishable_on_screen() {
    // codex round 5: two long hook prompts whose visible first row is
    // identical (they differ only in a wrapped continuation the ❯ pattern
    // never sees). The hold cannot tell them apart, so it must not wait
    // forever — after a few frames it trusts the screen and lets narration
    // through.
    let first_row = "please refactor the authentication module in the usual careful way";
    let mut tracker = ActivityTracker::new();
    tracker.begin_turn(&format!("{first_row} and also update the tests")); // turn 1
    tracker.observe(claude(), input(&screen(&format!("❯ {first_row}"))));
    tracker.begin_turn(&format!("{first_row} but leave the tests alone")); // turn 2
                                                                           // The screen only ever shows the shared first row; narration for turn 2
                                                                           // arrives but the prompt row is identical to turn 1's.
    let frame = screen(&format!("❯ {first_row}\n● Working on it…"));
    let mut released = false;
    for _ in 0..(HOLD_FRAMES as usize + 2) {
        tracker.observe(claude(), input(&frame));
        if tracker.current().map(|a| a.kind) == Some(ActivityKind::Narration) {
            released = true;
            break;
        }
    }
    assert!(released, "the hold never released and stayed on the prompt");
    assert_eq!(tracker.current(), narration("Working on it…").as_ref());
}
