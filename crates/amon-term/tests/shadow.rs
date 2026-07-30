//! The shadow terminal is only worth having if the vendored manifests reach the
//! same verdict against it that they would against the user's real terminal.
//! These drive it with the kind of output agents actually emit and assert on
//! the detection result, not on the intermediate text.

use amon_detect::{detect_agent_with_osc, Agent, AgentState};
use amon_term::ShadowTerminal;

fn shadow_of(output: &[&str]) -> ShadowTerminal {
    // Detection layers cached remote manifests over the bundled ones, and this
    // machine may have a cache. Point the state dir somewhere empty so tests
    // exercise exactly what this build ships.
    // SAFETY: single-threaded test process; nextest gives each test its own.
    unsafe {
        std::env::set_var(
            "XDG_STATE_HOME",
            std::env::temp_dir().join("amon-shadow-tests"),
        )
    };

    let mut shadow = ShadowTerminal::new(80, 24).expect("shadow terminal");
    for chunk in output {
        shadow.feed(chunk.as_bytes());
    }
    shadow
}

fn state_of(shadow: &ShadowTerminal) -> AgentState {
    detect_agent_with_osc(
        Some(Agent::Claude),
        &shadow.detection_text(),
        shadow.title().as_deref().unwrap_or_default(),
        "",
    )
    .state
}

#[test]
fn a_braille_spinner_in_the_title_reads_as_working() {
    // Claude signals progress with a braille glyph in the OSC title.
    let shadow = shadow_of(&["\x1b]0;⠋ doing something\x07", "thinking…\r\n"]);

    assert_eq!(shadow.title().as_deref(), Some("⠋ doing something"));
    assert_eq!(state_of(&shadow), AgentState::Working);
}

#[test]
fn a_selection_form_reads_as_blocked() {
    let shadow = shadow_of(&[
        "some earlier output\r\n",
        "────────────────────\r\n",
        "Do you want to proceed?\r\n",
        "❯ 1. Yes\r\n",
        "  2. No\r\n",
        "enter to select · esc to cancel · ↑/↓ to navigate\r\n",
    ]);

    assert_eq!(state_of(&shadow), AgentState::Blocked);
}

#[test]
fn a_codex_trust_dialog_reads_as_blocked() {
    // The first thing codex shows in a new directory. It needs a human, so
    // reporting it as idle hides exactly the situation `blocked` exists for
    // (vendor/patches/0003).
    let shadow = shadow_of(&[
        "You are running Codex in /tmp/scratch\r\n",
        "\r\n",
        "Do you trust the contents of this directory?\r\n",
        "❯ 1. Yes, continue\r\n",
        "  2. No, quit\r\n",
    ]);

    let detection = detect_agent_with_osc(
        Some(Agent::Codex),
        &shadow.detection_text(),
        shadow.title().as_deref().unwrap_or_default(),
        "",
    );
    assert_eq!(detection.state, AgentState::Blocked);
}

#[test]
fn a_claude_trust_dialog_reads_as_blocked() {
    let shadow = shadow_of(&[
        "Do you trust the files in this folder?\r\n",
        "/tmp/scratch\r\n",
        "❯ 1. Yes, proceed\r\n",
        "  2. No, exit\r\n",
    ]);

    assert_eq!(state_of(&shadow), AgentState::Blocked);
}

#[test]
fn an_empty_prompt_reads_as_idle() {
    let shadow = shadow_of(&["\x1b]0;✳ amon.sh\x07", "ready\r\n"]);

    assert_eq!(state_of(&shadow), AgentState::Idle);
}

#[test]
fn detection_text_is_the_visible_tail_without_trailing_blanks() {
    let shadow = shadow_of(&["first line\r\n", "second line\r\n", "\r\n\r\n\r\n"]);

    let text = shadow.detection_text();
    assert!(text.starts_with("first line\nsecond line\n"), "{text:?}");
    assert!(
        !text.trim_end_matches('\n').ends_with('\n'),
        "trailing blank rows are dropped: {text:?}"
    );
}

#[test]
fn detection_reads_one_screen_not_the_scrollback() {
    // herdr's detection window is the screen height; on a terminal shorter
    // than that, reading extra scrollback rows would let manifests match
    // output the user scrolled past long ago.
    let mut shadow = ShadowTerminal::new(80, 5).expect("shadow terminal");
    for line in 1..=8 {
        shadow.feed(format!("line {line}\r\n").as_bytes());
    }

    let text = shadow.detection_text();
    assert!(text.contains("line 8"), "{text:?}");
    assert!(
        !text.contains("line 3"),
        "rows above the screen are scrollback, not screen: {text:?}"
    );
}

#[test]
fn control_sequences_are_interpreted_rather_than_echoed() {
    // A cleared line must not leave its old contents in the detection text,
    // or manifests would match on output the user can no longer see.
    let shadow = shadow_of(&["stale content\r", "\x1b[2Kfresh content\r\n"]);

    let text = shadow.detection_text();
    assert!(text.contains("fresh content"), "{text:?}");
    assert!(!text.contains("stale content"), "{text:?}");
}

#[test]
fn resizing_tracks_the_real_terminal() {
    let mut shadow = shadow_of(&["hello\r\n"]);
    assert_eq!(shadow.size(), (80, 24));

    shadow.resize(120, 40).expect("resize");
    assert_eq!(shadow.size(), (120, 40));
    assert!(shadow.detection_text().contains("hello"));
}

#[test]
fn a_zero_sized_terminal_is_clamped_rather_than_rejected() {
    // SIGWINCH can report 0x0 while a terminal is being torn down; the wrapper
    // must not die because of it.
    let mut shadow = ShadowTerminal::new(0, 0).expect("clamped");
    assert_eq!(shadow.size(), (1, 1));
    shadow.resize(0, 0).expect("still clamped");
}
