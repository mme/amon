//! The bar widget's contract with Omarchy.
//!
//! These drive `desktop::install` and `desktop::uninstall` directly rather than
//! through the CLI. They used to run as `amon setup omarchy` / `amon remove
//! omarchy` end-to-end, but naming a desktop in the slot that otherwise names
//! an agent put two unlike operations behind one word, and that target is gone.
//! What it was really exercising was this module, so this is where it lives now.
//!
//! Each test gets its own `HOME` and its own `PATH`. The `PATH` matters as much
//! as the home: `uninstall` shells out to `omarchy`, a developer's machine may
//! well ship `/usr/bin/omarchy`, and a test that reached it would drive their
//! real desktop. Only what a test plants is reachable.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use amon_integration::desktop;
use amon_integration::{DesktopTarget, InstallState};

const TARGET: DesktopTarget = DesktopTarget::Omarchy;

/// Where Omarchy discovers third-party plugins, relative to `$HOME`.
const PLUGIN_DIR: &str = ".config/omarchy/plugins/sh.amon.workspaces";

struct Desk {
    root: PathBuf,
}

impl Desk {
    /// SAFETY for the `set_var`s: nextest gives every test its own process, so
    /// nothing else is reading the environment while this runs. That is the
    /// same reason the workspace uses nextest at all (see the justfile).
    fn new() -> Self {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let root = std::env::temp_dir().join(format!(
            "amon-desk{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("home")).expect("home");
        std::fs::create_dir_all(root.join("bin")).expect("bin");
        unsafe {
            std::env::set_var("HOME", root.join("home"));
            std::env::set_var("PATH", root.join("bin"));
        }
        Self { root }
    }

    fn home(&self, relative: &str) -> PathBuf {
        self.root.join("home").join(relative)
    }

    fn plugin(&self) -> PathBuf {
        self.home(PLUGIN_DIR)
    }

    fn scratch(&self, relative: &str) -> PathBuf {
        self.root.join(relative)
    }

    /// Plants the `omarchy` CLI this sandbox will find, replacing any earlier
    /// one — several tests install with a working CLI and then remove with a
    /// broken one.
    fn omarchy_cli(&self, script: &str) {
        let path = self.root.join("bin/omarchy");
        std::fs::write(&path, script).expect("write stub");
        set_executable(&path);
    }

    /// Takes the CLI away again: off Omarchy there is simply no `omarchy`.
    fn no_omarchy_cli(&self) {
        let _ = std::fs::remove_file(self.root.join("bin/omarchy"));
    }
}

fn set_executable(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let mut permissions = std::fs::metadata(path).expect("stat").permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(path, permissions).expect("chmod");
}

fn install() -> std::io::Result<Vec<String>> {
    desktop::install(TARGET)
}

fn uninstall() -> std::io::Result<Vec<String>> {
    desktop::uninstall(TARGET)
}

fn joined(messages: &[String]) -> String {
    messages.join("\n")
}

#[test]
fn installing_puts_the_widget_where_the_shell_looks() {
    let desk = Desk::new();

    let messages = install().expect("install succeeds");

    let plugin = desk.plugin();
    let manifest = std::fs::read_to_string(plugin.join("manifest.json")).expect("a manifest");
    let manifest: serde_json::Value = serde_json::from_str(&manifest).expect("valid json");

    // The contract Omarchy loads a plugin by. A manifest it cannot read is the
    // one failure with no symptom: the widget just never appears.
    assert_eq!(manifest["id"], "sh.amon.workspaces");
    assert_eq!(manifest["schemaVersion"], 1);
    assert_eq!(manifest["kinds"], serde_json::json!(["bar-widget"]));
    let entry = manifest["entryPoints"]["barWidget"]
        .as_str()
        .expect("entry");
    assert!(
        plugin.join(entry).exists(),
        "the manifest's entry point must be installed: {entry}"
    );

    // The clonedFrom declaration is what lets one enable command take the
    // built-in widget's own slot; without it the widget lands in some section
    // and the built-in stays.
    assert_eq!(manifest["omarchy"]["clonedFrom"], "omarchy.workspaces");

    let report = joined(&messages);
    assert!(
        report.contains("omarchy plugin enable sh.amon.workspaces"),
        "installing on its own does not enable, so it says how: {report}"
    );
    assert!(
        !report.contains("omarchy bar plugin"),
        "the pre-Quattro bar plugin commands no longer exist: {report}"
    );
}

#[test]
fn installing_leaves_the_bar_config_alone() {
    // shell.json becomes the user's canonical file once they touch it, and
    // Omarchy does not deep-merge — so amon writing into it could silently
    // discard a layout they chose.
    let desk = Desk::new();
    let shell_json = desk.home(".config/omarchy/shell.json");
    std::fs::create_dir_all(shell_json.parent().unwrap()).expect("config dir");
    std::fs::write(&shell_json, "{\"mine\":true}").expect("write shell.json");

    install().expect("install succeeds");

    assert_eq!(
        std::fs::read_to_string(&shell_json).expect("still there"),
        "{\"mine\":true}",
        "the user's bar config must be untouched"
    );
}

#[test]
fn reinstalling_restores_a_widget_someone_broke() {
    let desk = Desk::new();
    install().expect("install succeeds");

    let widget = desk.plugin().join("AgentStates.qml");
    let original = std::fs::read_to_string(&widget).expect("installed");
    std::fs::write(&widget, "corrupted").expect("corrupt it");

    install().expect("reinstall succeeds");

    assert_eq!(
        std::fs::read_to_string(&widget).expect("still there"),
        original,
        "reinstalling rewrites every file, not just the missing ones"
    );
}

#[test]
fn a_widget_someone_broke_is_not_reported_current() {
    // The point of reporting currency is catching a widget that no longer
    // matches the daemon it parses; a version string alone cannot do that.
    let desk = Desk::new();
    install().expect("install succeeds");
    std::fs::write(desk.plugin().join("Workspaces.qml"), "not what we shipped")
        .expect("corrupt it");

    let status = desktop::status(TARGET);

    assert_ne!(
        status.state,
        InstallState::Current,
        "a broken install must not claim to be current"
    );
}

#[test]
fn uninstalling_removes_amons_files_and_nothing_else() {
    let desk = Desk::new();
    install().expect("install succeeds");
    let plugin = desk.plugin();

    // Something the user put there. Whatever it is, it is not amon's to delete.
    let theirs = plugin.join("notes.md");
    std::fs::write(&theirs, "mine").expect("write");

    uninstall().expect("uninstall succeeds");

    for ours in ["manifest.json", "AgentStates.qml", "Workspaces.qml"] {
        assert!(!plugin.join(ours).exists(), "{ours} goes");
    }
    assert_eq!(
        std::fs::read_to_string(&theirs).expect("theirs survives"),
        "mine",
        "a file amon did not write survives, and keeps the directory alive"
    );
}

#[test]
fn uninstalling_asks_omarchy_to_restore_the_built_in_first() {
    // The shell's clonedFrom swap-back reads the manifest at disable time, so
    // the disable has to land while the file is still on disk — afterwards the
    // bar entry would be dropped without the built-in coming back. (Omarchy's
    // own `plugin remove` sequences itself the same way.)
    let desk = Desk::new();
    install().expect("install succeeds");

    let record = desk.scratch("omarchy-was-called");
    let manifest = desk.plugin().join("manifest.json");
    desk.omarchy_cli(&format!(
        "#!/bin/sh\nprintf '%s' \"$*\" > {record}\n\
         [ -f {manifest} ] && printf ' manifest-still-there' >> {record}\n",
        record = record.display(),
        manifest = manifest.display(),
    ));

    let messages = uninstall().expect("uninstall succeeds");

    assert_eq!(
        std::fs::read_to_string(&record).expect("the omarchy CLI was invoked"),
        "plugin disable sh.amon.workspaces manifest-still-there"
    );
    assert!(!manifest.exists(), "the files still go afterwards");
    assert!(
        joined(&messages).contains("restored the built-in"),
        "the user hears the bar was fixed up: {messages:?}"
    );
}

#[test]
fn a_failing_omarchy_cli_does_not_block_removal() {
    let desk = Desk::new();
    install().expect("install succeeds");
    desk.omarchy_cli("#!/bin/sh\nexit 1\n");

    let messages = uninstall().expect("uninstall succeeds anyway");

    assert!(
        !desk.plugin().join("manifest.json").exists(),
        "removal proceeds without the bar"
    );
    assert!(
        joined(&messages).contains("omarchy plugin disable"),
        "the manual fix-up is printed instead: {messages:?}"
    );
}

#[test]
fn a_machine_without_the_omarchy_cli_uninstalls_quietly() {
    // No CLI means no bar to fix up — that is the ordinary case off Omarchy,
    // not something to warn about.
    let desk = Desk::new();
    install().expect("install succeeds");
    desk.no_omarchy_cli();

    let messages = uninstall().expect("uninstall succeeds");

    assert!(!desk.plugin().join("manifest.json").exists());
    assert!(
        !joined(&messages).contains("could not"),
        "no warning where there is nothing to warn about: {messages:?}"
    );
}

#[test]
fn removing_a_pre_swap_widget_does_not_claim_restoration() {
    // A widget installed before the manifest declared clonedFrom disables
    // without swapping the built-in back; saying "restored" would hide a bar
    // that just lost its workspace indicators.
    let desk = Desk::new();
    desk.omarchy_cli("#!/bin/sh\nexit 0\n");
    install().expect("install succeeds");

    let manifest_path = desk.plugin().join("manifest.json");
    let mut manifest: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&manifest_path).expect("manifest"))
            .expect("valid json");
    manifest
        .as_object_mut()
        .expect("object")
        .remove("omarchy")
        .expect("the field to regress");
    std::fs::write(&manifest_path, manifest.to_string()).expect("regress the manifest");

    let messages = uninstall().expect("uninstall succeeds");

    let report = joined(&messages);
    assert!(
        !report.contains("(restored the built-in"),
        "no restore claim for a pre-swap copy: {report}"
    );
    assert!(
        report.contains("omarchy plugin enable omarchy.workspaces"),
        "the fix-up is named instead: {report}"
    );
}

#[test]
fn someone_elses_plugin_under_the_same_id_is_left_alone() {
    let desk = Desk::new();
    let plugin = desk.plugin();
    std::fs::create_dir_all(&plugin).expect("their dir");
    let theirs = r#"{"id":"someone.else"}"#;
    std::fs::write(plugin.join("manifest.json"), theirs).expect("their file");

    assert!(install().is_err(), "install refuses");
    assert!(uninstall().is_err(), "uninstall refuses");

    assert_eq!(
        std::fs::read_to_string(plugin.join("manifest.json")).expect("untouched"),
        theirs
    );
}

#[test]
fn a_symlinked_plugin_directory_is_left_alone() {
    // Dotfiles kept in a repository and linked into place. Writing through the
    // link would edit files in that repository; removing them would delete
    // them from it.
    let desk = Desk::new();
    let plugin = desk.plugin();
    let elsewhere = desk.scratch("dotfiles-widget");
    std::fs::create_dir_all(&elsewhere).expect("their repo");
    std::fs::write(elsewhere.join("manifest.json"), "theirs").expect("their file");
    std::fs::create_dir_all(plugin.parent().unwrap()).expect("plugins dir");
    std::os::unix::fs::symlink(&elsewhere, &plugin).expect("link it into place");

    assert!(install().is_err());
    assert!(uninstall().is_err());

    assert_eq!(
        std::fs::read_to_string(elsewhere.join("manifest.json")).expect("untouched"),
        "theirs"
    );
    assert!(elsewhere.exists(), "their directory survives");
}

#[test]
fn a_symlinked_widget_file_is_never_written_through() {
    // The same hazard one level down: the directory is amon's, but a file in
    // it points into someone's repository.
    let desk = Desk::new();
    install().expect("install succeeds");
    let plugin = desk.plugin();
    let theirs = desk.scratch("their-widget.qml");
    std::fs::write(&theirs, "their widget").expect("their file");
    std::fs::remove_file(plugin.join("Workspaces.qml")).expect("make room");
    std::os::unix::fs::symlink(&theirs, plugin.join("Workspaces.qml")).expect("link");

    assert!(install().is_err());

    assert_eq!(
        std::fs::read_to_string(&theirs).expect("untouched"),
        "their widget",
        "install must not follow a symlink and truncate what it points at"
    );
}

#[test]
fn an_install_that_died_half_way_can_be_finished() {
    // Assets are written before the manifest, so an interrupted install leaves
    // files with nothing identifying them. Refusing that directory would lock
    // amon out of its own half-finished work.
    let desk = Desk::new();
    install().expect("install succeeds");
    let manifest = desk.plugin().join("manifest.json");
    std::fs::remove_file(&manifest).expect("kill it half way");

    install().expect("the second install finishes the job");

    assert!(manifest.exists(), "install completes");
}

/// The widget's copy of the one ranking.
const AGENT_STATES: &str = include_str!("../src/assets/omarchy/AgentStates.qml");

/// The state names the widget draws, in the order it ranks them.
fn order_in_the_widget() -> Vec<String> {
    let line = AGENT_STATES
        .lines()
        .find(|line| {
            line.trim_start()
                .starts_with("readonly property var order:")
        })
        .expect("the widget ranks the states somewhere");
    line.split(['[', ']'])
        .nth(1)
        .expect("the ranking is an array literal")
        .split(',')
        .map(|state| state.trim().trim_matches('"').to_string())
        .collect()
}

/// The same ranking as Rust computes it, named the way the widget names them.
fn order_in_rust() -> Vec<String> {
    let at = |state, seen| amon_protocol::AgentEntry {
        id: "a".into(),
        agent: "claude".into(),
        state,
        state_since: 0,
        cwd: "/".into(),
        pid: 1,
        args: Vec::new(),
        hostname: "h".into(),
        started_at: 0,
        title: None,
        agent_session_id: None,
        agent_session_path: None,
        window: None,
        workspace: None,
        branch: None,
        project: None,
        subpath: None,
        focused: None,
        seen,
        herdr: None,
    };
    let mut agents = [
        at(amon_protocol::AgentState::Idle, Some(true)),
        at(amon_protocol::AgentState::Working, None),
        at(amon_protocol::AgentState::Blocked, None),
        at(amon_protocol::AgentState::Idle, Some(false)),
    ];
    agents.sort_by_key(|agent| agent.attention());
    agents
        .iter()
        .map(|agent| match (agent.state, agent.seen) {
            (amon_protocol::AgentState::Blocked, _) => "blocked",
            (amon_protocol::AgentState::Working, _) => "working",
            (amon_protocol::AgentState::Idle, Some(false)) => "done",
            _ => "idle",
        })
        .map(str::to_string)
        .collect()
}

#[test]
fn the_widget_ranks_states_the_way_rust_does() {
    // QML cannot call `AgentEntry::attention`, so the one ranking is written
    // twice and this is the only thing holding the copies together. They must
    // agree or the bar and `amon focus` answer differently for the same
    // workspace: a spinner drawn where the click lands on a finished agent.
    assert_eq!(
        order_in_the_widget(),
        order_in_rust(),
        "the widget's `order` array has drifted from AgentEntry::attention"
    );
}

/// The pane's header, which counts what the bar widget's model tallies. Shared
/// by the Super+A modal and the popped-out window, so this is the one file that
/// decides how either of them names a state.
const AGENTS_VIEW: &str = include_str!("../src/assets/omarchy/AgentsView.qml");

/// The counters `emptyCounts` starts every tally from.
fn counters_in_the_widget() -> Vec<String> {
    let line = AGENT_STATES
        .lines()
        .skip_while(|line| !line.contains("function emptyCounts()"))
        .find(|line| line.trim_start().starts_with("return ({"))
        .expect("emptyCounts returns an object literal");
    keys_of(line.split(['{', '}']).nth(1).expect("an object literal"))
}

/// The states the header has a word for, out of its `labels` block.
fn labels_in_the_header() -> Vec<String> {
    let block: String = AGENTS_VIEW
        .lines()
        .skip_while(|line| !line.contains("readonly property var labels:"))
        .skip(1)
        .take_while(|line| !line.trim_start().starts_with("})"))
        .collect::<Vec<_>>()
        .join(" ");
    assert!(!block.is_empty(), "the header labels its states somewhere");
    keys_of(&block)
}

/// The names on the left of the colons in a QML object literal's body.
fn keys_of(body: &str) -> Vec<String> {
    body.split(',')
        .filter_map(|field| field.split(':').next())
        .map(|name| name.trim().to_string())
        .filter(|name| !name.is_empty())
        .collect()
}

/// Every state the widget ranks must also be a counter it starts at zero.
///
/// `recompute` tallies with `tally[agent.state] += 1`, so a state that is
/// ranked but missing from `emptyCounts` increments `undefined` — which is
/// NaN, and NaN travels to the header as a perfectly ordinary number. Nothing
/// throws, nothing logs; the pane just says "NaN RUNNING". Only comparing the
/// two lists catches it.
#[test]
fn the_states_and_the_counters_are_the_same_set() {
    let mut counters = counters_in_the_widget();
    let mut states = order_in_the_widget();
    counters.sort();
    states.sort();
    assert_eq!(
        states, counters,
        "every ranked state is counted, and no more"
    );
}

/// And every one of them has a word in the header.
///
/// The header walks the model's ranking to build its line, so a state with no
/// label reaches the pane as "2 undefined". Same silent shape as the NaN
/// above, from the other file.
#[test]
fn the_header_has_a_word_for_every_state() {
    let mut labelled = labels_in_the_header();
    let mut states = order_in_the_widget();
    labelled.sort();
    states.sort();
    assert_eq!(
        states, labelled,
        "every ranked state is named in the header"
    );
}

/// The header's figures run most-urgent-first, like everything else in amon.
///
/// It reads the order off the model rather than writing it out, which is the
/// point — this pins that it still does, because hand-listing the states here
/// would compile and read fine while quietly ordering the line by hand.
#[test]
fn the_header_takes_its_order_from_the_ranking() {
    assert!(
        AGENTS_VIEW.contains("for (const state of view.agents.order)"),
        "the header walks the model's ranking to build its line"
    );
}

/// The pane the modal draws and the pane the window draws are one pane, so both
/// are built from one pair of figures.
///
/// They drifted once: the window kept a hardcoded height after the modal's
/// changed, and popping out resized the pane in front of the user. Nothing
/// failed, because no test in this workspace executes QML — the only thing that
/// can catch it is reading the source and insisting both sides name the same
/// property.
#[test]
fn the_window_and_the_modal_are_built_from_the_same_figures() {
    const PANEL: &str = include_str!("../src/assets/omarchy/AgentPanel.qml");

    for expected in [
        // The window takes them raw...
        "implicitWidth: root.paneWidth",
        "implicitHeight: root.paneHeight",
        // ...and the modal takes the same two, clamped to the screen. The clamp
        // is the one thing they cannot share: `panel` does not exist while the
        // modal is closed, and a window sized from it opens at its minimum.
        "cardWidth: Math.min(root.paneWidth",
        "cardHeight: Math.min(root.paneHeight",
    ] {
        assert!(
            PANEL.contains(expected),
            "the pane is sized by `{expected}`, so popping out moves it rather \
             than resizing it"
        );
    }
}

/// The modal respects exclusive zones, which is what puts it where the window
/// goes.
///
/// Hyprland centres windows within the usable area, always — `center`, `move`
/// with percentages, every form was tried. So a modal spanning the whole screen
/// centres half the bar's height above the window it pops out into, and the
/// pane visibly jumps. Ignoring the bar here would put that half-height into a
/// compositor rule instead, where it would be wrong the moment the bar changed.
#[test]
fn the_modal_centres_where_a_window_centres() {
    const PANEL: &str = include_str!("../src/assets/omarchy/AgentPanel.qml");
    assert!(
        PANEL.contains("exclusionMode: ExclusionMode.Normal"),
        "the modal is inset by the bar, like a window is"
    );
}

/// The window is re-sized every time it opens, not just the first time.
///
/// It outlives being closed — that is what `keepLoaded` buys, and what lets the
/// modal shut without taking the window down with it. So it carries whatever
/// geometry it last had: tile it with Super+T, or drag an edge, and
/// `implicitWidth` stops applying, because implicit size is an initial value
/// rather than a standing instruction. Without an assignment on the way in,
/// reopening hands back the tile's dimensions and popping out resizes the pane
/// in front of the user — the same symptom as the drift above, from a cause no
/// amount of sharing figures can fix.
#[test]
fn the_window_takes_the_pane_size_every_time_it_opens() {
    const PANEL: &str = include_str!("../src/assets/omarchy/AgentPanel.qml");
    for expected in [
        "popped.width = root.paneWidth",
        "popped.height = root.paneHeight",
    ] {
        assert!(
            PANEL.contains(expected),
            "the window is re-sized on show: `{expected}`"
        );
    }
}

/// Getting to an agent goes through `amon focus`, from everywhere.
///
/// Reaching an agent is one operation with more than one part: the window it
/// is in, and — when it lives in a herdr pane — the pane inside that window.
/// The panel used to dispatch the compositor itself, which was a second
/// implementation of the jump, and it arrived at the right window with the
/// wrong pane in front of it. Nothing failed; you simply landed next to the
/// agent you asked for.
///
/// So: no widget names an agent's window to the compositor. `address:0x` is
/// how an agent window is named, and only amon may say it. The panel's own
/// pop-out window is matched by title, which is why the check is this
/// specific rather than a ban on `hyprctl`; the bar's workspace fallback is
/// a place, not an agent, and stays.
#[test]
fn every_way_to_an_agent_goes_through_amon_focus() {
    const PANEL: &str = include_str!("../src/assets/omarchy/AgentPanel.qml");
    const WORKSPACES: &str = include_str!("../src/assets/omarchy/Workspaces.qml");

    assert!(
        PANEL.contains(r#"["amon", "focus", "--agent""#),
        "the panel asks amon to go to the agent it picked"
    );
    for (name, source) in [
        ("AgentPanel.qml", PANEL),
        ("AgentStates.qml", AGENT_STATES),
        ("AgentsView.qml", AGENTS_VIEW),
        ("Workspaces.qml", WORKSPACES),
    ] {
        assert!(
            !source.contains("address:0x"),
            "{name} names an agent's window to the compositor itself; that is \
             a second way to reach an agent, and it will forget the herdr pane"
        );
    }
}

/// The pane and `amon status` round ages the same way.
///
/// The rule now exists twice — once in Rust for the CLI, once in QML because
/// the pane cannot call it — and two implementations of one rule drift. A row
/// reading "1h" beside a `amon status` line reading "59m" for the same agent
/// would be a small thing that quietly makes both untrustworthy.
#[test]
fn the_pane_ages_agents_the_way_the_cli_does() {
    const CLI: &str = include_str!("../../amon-cli/src/main.rs");

    // Seconds below a minute, minutes below an hour, hours after that.
    for boundary in ["0..=59", "60..=3599"] {
        assert!(
            CLI.contains(boundary),
            "the CLI's age rule still reads {boundary}"
        );
    }
    for boundary in ["seconds < 60", "seconds < 3600"] {
        assert!(
            AGENT_STATES.contains(boundary),
            "the pane's age rule still reads {boundary}"
        );
    }
    for unit in ["\"s\"", "\"m\"", "\"h\""] {
        assert!(AGENT_STATES.contains(unit), "the pane still names {unit}");
    }
}

/// The pane navigates the way the rest of the desktop does, plus Emacs.
///
/// Arrows and vi's j/k come from Omarchy's own PanelKeyCatcher rather than from
/// a hand-written copy, so the pane keeps answering whatever that component
/// answers. It has no Emacs bindings, which is the one thing added on top.
///
/// The FocusScope is the load-bearing part and the least obvious. PanelKeyCatcher
/// declares `focus: true`, which does nothing inside a plain Item: the host takes
/// active focus on the view itself and the catcher never receives a key. That
/// exact mistake shipped in this file once — arrows and j/k were dead while
/// Ctrl-N still worked, because the Ctrl chords are handled on the view and
/// everything else on the child.
#[test]
fn the_pane_answers_arrows_vi_and_emacs() {
    const VIEW: &str = include_str!("../src/assets/omarchy/AgentsView.qml");

    assert!(
        VIEW.contains("FocusScope {"),
        "the view is a FocusScope, or its key catcher never gets focus"
    );
    assert!(
        VIEW.contains("PanelKeyCatcher {"),
        "arrows and j/k come from Omarchy's own dispatcher"
    );
    for chord in ["Qt.Key_N", "Qt.Key_P", "Qt.ControlModifier"] {
        assert!(VIEW.contains(chord), "Emacs navigation still reads {chord}");
    }
}

/// The pane's model copies whole agents out of the daemon's entries by hand, so
/// a field added to the wire is not a field the pane can draw until it is
/// copied here too.
///
/// Missing one fails in the quietest possible way: the column renders, aligned
/// and empty, for every agent forever, and everything around it looks correct.
#[test]
fn the_model_copies_every_field_the_rows_draw() {
    const VIEW: &str = include_str!("../src/assets/omarchy/AgentsView.qml");

    // What `remember` copies out of the daemon's entry.
    let copied: Vec<String> = AGENT_STATES
        .lines()
        .skip_while(|line| !line.contains("root.agents[entry.id] = {"))
        .take_while(|line| !line.trim_start().starts_with("}"))
        .filter_map(|line| line.split(':').next())
        .map(|name| name.trim().to_string())
        .filter(|name| !name.is_empty() && !name.starts_with("//"))
        .collect();
    assert!(copied.contains(&"branch".to_string()), "copied: {copied:?}");

    // Every `row.entry.<field>` a row draws has to be one of them.
    for (index, _) in VIEW.match_indices("row.entry.") {
        let field: String = VIEW[index + "row.entry.".len()..]
            .chars()
            .take_while(|character| character.is_ascii_alphanumeric() || *character == '_')
            .collect();
        assert!(
            copied.contains(&field),
            "a row draws row.entry.{field}, which the model never copies: {copied:?}"
        );
    }
}

/// A QML function, from its `function name(` line to the brace that closes
/// it. The other extractors here take lines up to a closing token, which works
/// until the body has blocks of its own; this one counts braces instead.
fn qml_function<'a>(source: &'a str, name: &str) -> &'a str {
    let start = source
        .find(name)
        .unwrap_or_else(|| panic!("{name} exists in the QML"));
    let mut depth = 0i32;
    for (offset, character) in source[start..].char_indices() {
        match character {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return &source[start..start + offset + 1];
                }
            }
            _ => {}
        }
    }
    panic!("{name} has a body that closes");
}

/// The pane opens with its cursor on the agent `amon focus` would land on:
/// most urgent rank first, and within a rank the one that has been waiting
/// longest.
///
/// The key exists twice — once in Rust for Super+N, once in QML because the
/// pane cannot call it — and two copies of one rule drift. If they disagree,
/// Super+A and Super+N answer "what wants me most" differently, and whichever
/// one you trusted second taught you to trust neither.
#[test]
fn the_pane_opens_where_focus_would_land() {
    const FOCUS: &str = include_str!("../../amon-cli/src/focus.rs");
    assert!(
        FOCUS.contains(".min_by_key(|agent| (agent.attention(), agent.state_since))"),
        "focus still picks by (attention, state_since)"
    );

    let pick = qml_function(AGENT_STATES, "function initialIndex(");
    let rank = pick
        .find("root.rank(")
        .expect("the opening pick reads the ranking");
    let since = pick
        .find("stateSince")
        .expect("the opening pick breaks ties by stateSince");
    assert!(
        rank < since,
        "rank decides first and stateSince only between equals, like focus"
    );
}

/// The opening pick never lands on the agent you are already looking at, and
/// with nothing urgent it rotates: the row after yours, wrapping past the end.
///
/// Both halves come from one fact — you opened the pane to go somewhere else.
/// Skipping where you are is what makes Enter mean that; the wrap is what
/// keeps three idle agents from being "the first one, every time". The
/// rotation is deliberately stateless: where the cursor lands is a function of
/// where you are, never of how often the pane has been opened.
///
/// "Where you are" is the focused *window* when a row's window is the one
/// focused, and only falls back to the workspace when none is. Activation is
/// per-window — Enter focuses one agent's terminal — so a workspace-only skip
/// cannot cycle between two agents sharing a workspace: it skips both, and
/// every open lands on the same row. Two agents share a workspace; they never
/// share a window.
///
/// And the panel has to know where you are for any of it to act. Only the bar
/// widget's AgentStates learns the focused workspace from Hyprland; the
/// panel's own instance has to bind that and the focused window too, or both
/// stay "" forever and the pick quietly degrades to "row 0" — nothing fails,
/// the cursor is just always at the top and nobody can say why.
#[test]
fn the_opening_pick_goes_somewhere_other_than_where_you_are() {
    let pick = qml_function(AGENT_STATES, "function initialIndex(");
    assert!(
        pick.contains("focusedWindow"),
        "the pick knows which agent's window you are looking at"
    );
    assert!(
        pick.contains("focusedWorkspace"),
        "the pick falls back to the workspace when no agent's window is focused"
    );
    assert!(
        pick.contains("% rows.length"),
        "idle agents rotate from where you are, wrapping"
    );

    const PANEL: &str = include_str!("../src/assets/omarchy/AgentPanel.qml");
    assert!(
        PANEL.contains("focusedWorkspace: Hyprland.focusedWorkspace"),
        "the panel's model learns the focused workspace, like the bar's does"
    );
    assert!(
        PANEL.contains("Hyprland.activeToplevel"),
        "the panel's model learns the focused window from Hyprland"
    );
}

/// Every way into the pane re-picks the cursor; nothing carries a stale one.
///
/// The modal re-picks on each open because each open is a fresh "what needs me
/// now" — the answer from ten minutes ago is the one thing it must not be. The
/// window picks when it shows, which is once, so it keeps its cursor
/// thereafter exactly as before.
#[test]
fn every_way_into_the_pane_resets_the_cursor() {
    assert!(
        AGENTS_VIEW.contains("function resetSelection()"),
        "the view can re-pick its cursor"
    );
    assert!(
        AGENTS_VIEW.contains("view.agents.initialIndex()"),
        "the re-pick asks the model, which owns the rule"
    );

    const PANEL: &str = include_str!("../src/assets/omarchy/AgentPanel.qml");
    assert!(
        PANEL.contains("modalView.resetSelection()"),
        "the modal re-picks on every open"
    );
    assert!(
        PANEL.contains("windowView.resetSelection()"),
        "the window picks when it shows"
    );
}

#[test]
fn the_pane_hears_about_a_newer_release_by_the_daemons_name() {
    // The daemon's side of the wire — the check, the strictness, the replay
    // to late subscribers — is covered in amon-cli's e2e suite. These hold
    // the QML to the same event name and fields; a drift here would not
    // fail loudly, the footer would simply never appear.
    const EVENT: &str = include_str!("../../amon-protocol/src/event.rs");
    assert!(
        EVENT.contains(r#"rename = "update_available""#),
        "the name the daemon sends under"
    );
    assert!(
        AGENT_STATES.contains(r#"frame.event === "update_available""#),
        "the model listens for the same name"
    );
    assert!(
        AGENT_STATES.contains("params.installed") && AGENT_STATES.contains("params.latest"),
        "and reads the fields the event carries"
    );
}

#[test]
fn the_players_role_is_the_one_the_ducking_dropin_ranks() {
    // Three parties have to agree on one word: the daemon tags its stream,
    // the drop-in routes that tag onto the Notification bus, and the bus's
    // priority is what ducks the music. A drift would not fail loudly —
    // sounds would simply stop ducking anything.
    const SOUND: &str = include_str!("../../amon-daemon/src/sound.rs");
    const DROPIN: &str = include_str!("../src/assets/wireplumber/50-amon-ducking.conf");
    assert!(
        SOUND.contains(r#"media.role = "Notification""#),
        "the daemon tags its stream"
    );
    assert!(
        DROPIN.contains(r#"device.intended-roles = [ "Notification" ]"#),
        "the drop-in routes that tag"
    );
    assert!(
        DROPIN.contains(r#"policy.role-based.action.lower-priority = "duck""#),
        "and the notification bus ducks what sits below it"
    );
}

#[test]
fn a_known_update_does_not_survive_a_reconnect() {
    // `reset` runs on every connection state change. Versions cleared there
    // cannot go stale: a daemon restarted after its own upgrade replays the
    // event if there is still anything to say.
    let reset = qml_function(AGENT_STATES, "function reset()");
    assert!(
        reset.contains("installedVersion") && reset.contains("latestVersion"),
        "reset clears the update: {reset}"
    );
}

#[test]
fn super_ws_question_and_the_panels_answer_are_one_word() {
    // The bind matches on the literal string "dismissed"; the panel's
    // function returns it. A drift would not fail loudly — Super+W would
    // just go back to closing the window under the open pane (#39).
    const BINDINGS: &str = include_str!("../src/bindings.rs");
    const PANEL: &str = include_str!("../src/assets/omarchy/AgentPanel.qml");
    assert!(
        PANEL.contains("function dismissIfOpen()"),
        "the panel answers the question"
    );
    assert!(
        PANEL.contains(r#"return "dismissed""#),
        "with the word the bind matches on"
    );
    assert!(
        BINDINGS.contains("call sh.amon.panel dismissIfOpen"),
        "the bind asks that exact method"
    );
    assert!(
        BINDINGS.contains(r#"!= dismissed"#),
        "and matches that exact word"
    );
}

#[test]
fn the_update_footer_offers_the_installers_own_line() {
    // The command the footer shows (and copies) is the installer's own
    // masthead line, held here to the script itself so the pane can never
    // advertise a command that stopped working.
    const INSTALL: &str = include_str!("../../../install.sh");
    const COMMAND: &str = "curl -fsSL amon.sh/install | sh";
    assert!(
        INSTALL.contains(COMMAND),
        "install.sh documents the line the footer shows"
    );
    assert!(
        AGENTS_VIEW.contains(COMMAND),
        "the footer shows that exact line"
    );
    assert!(
        AGENTS_VIEW.contains("view.agents.updateAvailable"),
        "and only when the shared model says there is an update"
    );
    assert!(
        AGENTS_VIEW.contains("wl-copy"),
        "clicking copies rather than asking the user to retype"
    );
}

#[test]
fn the_spinner_ticks_for_every_working_agent() {
    // stateByWorkspace keeps only each workspace's most urgent state, and
    // blocked outranks working — so a workspace holding both hid its working
    // agent from the old anyWorking scan, and the panel's spinners froze
    // exactly while an agent waited for input (#38). The per-agent tally is
    // the honest source: a working agent counts wherever it sits.
    let any_working = AGENT_STATES
        .lines()
        .find(|line| line.contains("property bool anyWorking:"))
        .expect("anyWorking exists in the QML");
    assert!(
        any_working.contains("counts.working"),
        "anyWorking derives from the per-agent tally: {any_working}"
    );
    assert!(
        !any_working.contains("stateByWorkspace"),
        "and never from the per-workspace maxima, which blocked dominates: {any_working}"
    );
}
