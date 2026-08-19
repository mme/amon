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
