//! Desktop integrations — amon's own, beside the vendored agent installer.
//!
//! An agent Integration writes a hook into an agent's config so the agent
//! reports to amon. A desktop Integration is the other direction: an artifact
//! that *subscribes*, showing amon's state where the user already looks. Both
//! are installed by `amon setup` and both report whether the copy on
//! disk is current, which is the whole reason they share vocabulary.
//!
//! It lives here rather than as another `IntegrationTarget`: that list is
//! consumed by vendored herdr code matching exhaustively on every entry to pick
//! a hook asset and an agent config to edit (ADR-0005), and a bar widget has
//! neither. Wedging it in would break those matches and put a member into a set
//! whose machinery means nothing for it.

use std::io;
use std::path::{Path, PathBuf};

use crate::api_surface::InstallState;

/// A desktop amon can install an Integration for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DesktopTarget {
    /// The Omarchy shell's bar (Quickshell), Quattro and later.
    Omarchy,
}

impl DesktopTarget {
    pub const ALL: [Self; 1] = [Self::Omarchy];

    pub fn label(self) -> &'static str {
        match self {
            Self::Omarchy => "omarchy",
        }
    }

    /// The plugin id Omarchy discovers this by, and the directory name it
    /// lives in.
    fn plugin_id(self) -> &'static str {
        match self {
            Self::Omarchy => "sh.amon.workspaces",
        }
    }
}

const MANIFEST: &str = include_str!("assets/omarchy/manifest.json");
const WORKSPACES_QML: &str = include_str!("assets/omarchy/Workspaces.qml");
const AGENT_STATES_QML: &str = include_str!("assets/omarchy/AgentStates.qml");

const MANIFEST_NAME: &str = "manifest.json";

/// The widget's own files. Written manifest-last and removed manifest-first,
/// because the manifest is what everything else keys off: it is what Omarchy
/// loads the plugin by and what [`status`] reads a version out of, so a
/// half-written plugin must never be one that carries a current manifest.
const ASSETS: [(&str, &str); 2] = [
    ("Workspaces.qml", WORKSPACES_QML),
    ("AgentStates.qml", AGENT_STATES_QML),
];

/// What the widget on disk is compared against. The shipped manifest is the
/// single source of truth for it — there is no second place to forget to bump.
pub fn expected_version() -> String {
    manifest_version(MANIFEST).unwrap_or_default()
}

fn manifest_version(manifest: &str) -> Option<String> {
    serde_json::from_str::<serde_json::Value>(manifest)
        .ok()?
        .get("version")?
        .as_str()
        .map(str::to_owned)
}

/// `~/.config/omarchy`, the way Omarchy itself resolves it.
///
/// Deliberately not `XDG_CONFIG_HOME`: Omarchy's plugin registry builds its
/// search path as `HOME + "/.config/omarchy/plugins"` and never consults the
/// XDG variable, so honouring it here would install a plugin somewhere the
/// shell will never look.
fn omarchy_config_dir() -> Option<PathBuf> {
    let home = std::env::var_os("HOME").filter(|home| !home.is_empty())?;
    Some(PathBuf::from(home).join(".config").join("omarchy"))
}

/// Where the plugin's own directory goes.
pub fn plugin_dir(target: DesktopTarget) -> Option<PathBuf> {
    Some(
        omarchy_config_dir()?
            .join("plugins")
            .join(target.plugin_id()),
    )
}

fn missing_home() -> io::Error {
    io::Error::other("cannot find the Omarchy config directory: HOME is not set")
}

/// Whether a directory already there is one amon may write into.
///
/// It is amon's if it holds a manifest naming this plugin — that is what
/// install writes and nothing else has reason to. A directory holding
/// something else, or reached through a symlink (a dotfiles checkout linked
/// into place), is left alone: overwriting or deleting it would destroy
/// someone's files under a plugin id that merely happens to match.
fn ours(directory: &Path) -> io::Result<bool> {
    let metadata = match std::fs::symlink_metadata(directory) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(true),
        Err(error) => return Err(error),
    };
    if metadata.file_type().is_symlink() {
        return Ok(false);
    }

    // No file amon writes may be a symlink: writing one follows it and
    // truncates whatever it points at, which for a dotfiles checkout is
    // someone else's file in someone else's repository.
    for name in file_names() {
        if let Ok(metadata) = std::fs::symlink_metadata(directory.join(name)) {
            if metadata.file_type().is_symlink() {
                return Ok(false);
            }
        }
    }

    match std::fs::read_to_string(directory.join(MANIFEST_NAME)) {
        Ok(installed) => Ok(manifest_id(&installed).as_deref() == manifest_id(MANIFEST).as_deref()),
        // Nothing to identify it by, so go on what is there: a directory
        // holding only files amon writes is amon's, however it came to be that
        // way. That covers one someone precreated, and an install or uninstall
        // that died half way — which writes the manifest last and removes it
        // first, and would otherwise have locked itself out of finishing.
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            for entry in std::fs::read_dir(directory)? {
                let found = entry?.file_name();
                if !file_names().any(|ours| ours == found) {
                    return Ok(false);
                }
            }
            Ok(true)
        }
        Err(error) => Err(error),
    }
}

/// Every file amon writes into the plugin directory.
fn file_names() -> impl Iterator<Item = &'static str> {
    std::iter::once(MANIFEST_NAME).chain(ASSETS.iter().map(|(name, _)| *name))
}

fn manifest_id(manifest: &str) -> Option<String> {
    serde_json::from_str::<serde_json::Value>(manifest)
        .ok()?
        .get("id")?
        .as_str()
        .map(str::to_owned)
}

fn not_ours(directory: &Path) -> io::Error {
    io::Error::other(format!(
        "{} exists and was not written by amon; leaving it alone",
        directory.display()
    ))
}

/// Writes the plugin, returning the notes to print — including the one Omarchy
/// command the user runs to enable it.
///
/// The manifest declares `omarchy.clonedFrom: omarchy.workspaces`, so enabling
/// swaps the widget into the built-in's own bar slot — same section, same
/// index, settings carried over — and disabling swaps the built-in back. That
/// is Quattro's clone mechanism, honored for any manifest that declares it.
///
/// Idempotent: every file is rewritten, and nothing else in the Omarchy config
/// is touched. In particular `shell.json` is left alone; it becomes the user's
/// canonical file the moment they edit it, and Omarchy does not deep-merge, so
/// amon writing into it could silently discard bar layout the user chose.
pub fn install(target: DesktopTarget) -> io::Result<Vec<String>> {
    let directory = plugin_dir(target).ok_or_else(missing_home)?;
    if !ours(&directory)? {
        return Err(not_ours(&directory));
    }
    std::fs::create_dir_all(&directory)?;
    // Manifest last: until it is there the plugin is not loadable and not
    // reported current, so a write that fails part way leaves an installation
    // that admits to being incomplete.
    for (name, contents) in ASSETS {
        std::fs::write(directory.join(name), contents)?;
    }
    std::fs::write(directory.join(MANIFEST_NAME), MANIFEST)?;

    let id = target.plugin_id();
    Ok(vec![
        format!("installed {} to {}", id, directory.display()),
        String::new(),
        "Enable it — it takes the built-in workspaces widget's place on the bar:".to_string(),
        format!("  omarchy plugin enable {id}"),
        String::new(),
        "(disabling it puts the built-in back)".to_string(),
    ])
}

/// Removes exactly what [`install`] wrote — after asking the omarchy CLI to
/// disable the widget, which must happen while the manifest is still on disk:
/// the shell's `clonedFrom` swap-back reads it at disable time, so disabling
/// after deletion would drop the bar entry without restoring the built-in.
/// (`omarchy plugin remove` sequences itself the same way.)
pub fn uninstall(target: DesktopTarget) -> io::Result<Vec<String>> {
    let directory = plugin_dir(target).ok_or_else(missing_home)?;
    if std::fs::symlink_metadata(&directory).is_err() {
        return Ok(vec![format!("{} is not installed", target.plugin_id())]);
    }
    if !ours(&directory)? {
        return Err(not_ours(&directory));
    }

    let id = target.plugin_id();
    // The swap-back only happens when the *installed* manifest declares
    // `clonedFrom` — a copy from before the manifest gained it disables
    // without restoring anything, and claiming otherwise would hide a bar
    // that just lost its workspace indicators. Judged from the disk manifest:
    // the shell's cached copy could in principle lag it, but only within the
    // watcher's debounce of a rewrite — and setup and remove are separate
    // human-run commands, not milliseconds apart. Accepted.
    let swaps_back = std::fs::read_to_string(directory.join(MANIFEST_NAME))
        .ok()
        .and_then(|manifest| serde_json::from_str::<serde_json::Value>(&manifest).ok())
        .is_some_and(|manifest| manifest["omarchy"]["clonedFrom"].is_string());
    let restore_note = disable_widget(id, swaps_back);

    // Manifest first, so an interrupted removal leaves something the shell no
    // longer loads rather than a plugin missing its widget.
    for name in file_names() {
        match std::fs::remove_file(directory.join(name)) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    // Plain `remove_dir`: it fails when anything else is in there, which is
    // exactly the outcome wanted — whatever that is, it is not amon's.
    let _ = std::fs::remove_dir(&directory);

    let mut notes = vec![format!("removed {} from {}", id, directory.display())];
    notes.extend(restore_note);
    Ok(notes)
}

/// Runs `omarchy plugin disable <id>`, returning the notes to print about it.
///
/// Best-effort by design: removal must not be blocked by a bar that cannot be
/// reached. No omarchy CLI means no bar to fix up, so that says nothing; a CLI
/// that fails (shell not running, stale id) gets the manual commands — which
/// name the *built-in*, because by the time anyone runs them this plugin's
/// manifest is gone and only re-enabling `omarchy.workspaces` still works.
fn disable_widget(id: &str, swaps_back: bool) -> Vec<String> {
    match std::process::Command::new("omarchy")
        .args(["plugin", "disable", id])
        .output()
    {
        // The one outcome here that is a success rather than an explanation,
        // and the only one that is a single line — which is how the caller
        // tells them apart and gives this one a tick of its own.
        Ok(output) if output.status.success() && swaps_back => {
            vec!["restored the built-in workspaces widget".to_string()]
        }
        // Disabled, but nothing swapped back: the installed copy predates the
        // clonedFrom manifest, so the shell only dropped the bar entry.
        Ok(output) if output.status.success() => vec![
            "the installed widget predates the in-place swap, so the built-in".to_string(),
            "was not restored; if the bar is missing its workspaces:".to_string(),
            "  omarchy plugin enable omarchy.workspaces".to_string(),
        ],
        Err(error) if error.kind() == io::ErrorKind::NotFound => Vec::new(),
        Ok(_) | Err(_) => vec![
            "could not ask omarchy to restore the built-in widget; if the bar".to_string(),
            "is missing its workspaces:".to_string(),
            format!("  omarchy plugin disable {id}"),
            "  omarchy plugin enable omarchy.workspaces".to_string(),
        ],
    }
}

/// Whether the `omarchy` CLI is on this machine — the gate for offering the
/// widget at all, and for anything below that shells out to it.
pub fn cli_present() -> bool {
    crate::api_surface::on_path("omarchy")
}

/// Restarts the Omarchy shell, so widget files replaced under a running one
/// become the files it is actually drawing.
///
/// [`enable`] already asks the shell to rescan, and a rescan looks like it
/// should be enough: it unregisters every plugin widget, calls
/// `Qt.clearComponentCache()`, and scans again. It is not. Measured on a shell
/// answering IPC normally: rewriting both the entry component and the nested
/// import it pulls in, then rescanning, left the bar drawing the previous copy
/// of each. Only a restart picked them up.
///
/// A heavy instrument for one widget — it takes the notifications and the OSD
/// with it — so this is for callers that have actually replaced files under a
/// running shell, never for a first install, which has nothing cached to be
/// stale. Worth raising upstream: a plugin author would reasonably expect the
/// rescan to be the whole story, and if it becomes so, this goes away.
pub fn restart_shell() -> io::Result<()> {
    let output = std::process::Command::new("omarchy")
        .args(["restart", "shell"])
        .output()?;
    if !output.status.success() {
        return Err(io::Error::other(format!(
            "omarchy restart shell failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(())
}

/// Runs `omarchy plugin enable` for the installed widget. The manifest's
/// `clonedFrom` makes that one command the whole story: the shell swaps the
/// widget into the built-in's bar slot.
///
/// Discovery first, though: the shell finds plugins via a debounced file
/// watcher, and `omarchy plugin enable` does not rescan — a plugin written
/// milliseconds ago fails with "not known". Omarchy's own clone script
/// rescans and polls before enabling for exactly this reason; this mirrors
/// it, best-effort, and lets enable produce the canonical error if discovery
/// still has not happened.
pub fn enable(target: DesktopTarget) -> io::Result<Vec<String>> {
    let id = target.plugin_id();

    let _ = std::process::Command::new("omarchy-shell")
        .args(["shell", "rescanPlugins"])
        .output();
    for _ in 0..40 {
        // Only keep waiting while the shell answers with a real list that
        // does not yet show the *shipped* manifest: that is the one state
        // polling can improve. Mere id presence is not enough — an upgrade
        // of an already-known widget would pass instantly while the shell's
        // cached manifest still lacks `clonedFrom`, and enable would then
        // place the widget beside the built-in instead of swapping it in.
        // No CLI, an error, or unparseable output ends the wait immediately.
        match discovered_current(id) {
            Some(false) => std::thread::sleep(std::time::Duration::from_millis(50)),
            _ => break,
        }
    }

    let output = std::process::Command::new("omarchy")
        .args(["plugin", "enable", id])
        .output()?;
    if !output.status.success() {
        return Err(io::Error::other(format!(
            "omarchy plugin enable {id} failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(vec![
        "enabled on your bar, in the built-in workspaces widget's place".to_string(),
    ])
}

/// Whether the shell's registry knows this plugin id *as currently shipped*:
/// the listed entry must carry the manifest's `clonedFrom`, which is how a
/// freshly written upgrade is told apart from a stale cached registration.
/// `None` when the list cannot be read (or predates the `clonedFrom` field in
/// its output, which reads as never-current and simply exhausts the poll).
fn discovered_current(id: &str) -> Option<bool> {
    let source = manifest_clone_source();
    Some(
        plugin_list()?
            .iter()
            .any(|plugin| plugin["id"] == id && plugin["clonedFrom"] == source.as_str()),
    )
}

/// The `omarchy.clonedFrom` the shipped manifest declares.
fn manifest_clone_source() -> String {
    serde_json::from_str::<serde_json::Value>(MANIFEST)
        .ok()
        .and_then(|manifest| {
            manifest["omarchy"]["clonedFrom"]
                .as_str()
                .map(str::to_owned)
        })
        .unwrap_or_default()
}

fn plugin_list() -> Option<Vec<serde_json::Value>> {
    let output = std::process::Command::new("omarchy")
        .args(["plugin", "list", "--json"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let plugins: serde_json::Value = serde_json::from_slice(&output.stdout).ok()?;
    plugins.as_array().cloned()
}

/// Whether the widget currently sits in the bar, asked of the running shell.
/// `None` when there is no answer to be had — no CLI, no shell, unparseable
/// output — which a caller reports as "unknown", never as "no".
pub fn enabled_in_bar(target: DesktopTarget) -> Option<bool> {
    // A list that parses but lacks the widget is a real answer — not
    // installed, so not in the bar — never "unknown".
    Some(
        plugin_list()?
            .iter()
            .find(|plugin| plugin["id"] == target.plugin_id())
            .and_then(|plugin| plugin["enabled"].as_bool())
            .unwrap_or(false),
    )
}

#[derive(Debug, Clone)]
pub struct DesktopStatus {
    pub target: DesktopTarget,
    pub label: &'static str,
    pub path: PathBuf,
    pub state: InstallState,
    pub installed_version: Option<String>,
    pub expected_version: String,
}

pub fn statuses() -> Vec<DesktopStatus> {
    DesktopTarget::ALL.into_iter().map(status).collect()
}

pub fn status(target: DesktopTarget) -> DesktopStatus {
    let directory = plugin_dir(target).unwrap_or_default();
    let installed = installed_version(&directory);
    let expected = expected_version();
    let state = match &installed {
        None => InstallState::NotInstalled,
        // The version alone is not enough: a widget file that failed to write,
        // was truncated, or was edited by hand would otherwise be reported
        // current, which is the one thing this is here to prevent.
        Some(version) if *version == expected && files_match(&directory) => InstallState::Current,
        Some(_) => InstallState::Outdated,
    };

    DesktopStatus {
        target,
        label: target.label(),
        path: directory,
        state,
        installed_version: installed,
        expected_version: expected,
    }
}

fn installed_version(directory: &Path) -> Option<String> {
    let manifest = std::fs::read_to_string(directory.join(MANIFEST_NAME)).ok()?;
    manifest_version(&manifest)
}

/// Whether every file on disk is byte-for-byte the one this build ships —
/// the manifest included, since a manifest whose version still matches but
/// whose `kinds` or `entryPoints` were mangled is one Omarchy may refuse to
/// load, and saying "current" about that helps nobody.
fn files_match(directory: &Path) -> bool {
    std::iter::once((MANIFEST_NAME, MANIFEST))
        .chain(ASSETS)
        .all(|(name, contents)| {
            std::fs::read_to_string(directory.join(name)).is_ok_and(|found| found == contents)
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_shipped_manifest_says_what_omarchy_requires() {
        // A manifest Omarchy cannot read is the one failure with no symptom:
        // the widget simply never appears, and nothing reports why.
        let manifest: serde_json::Value =
            serde_json::from_str(MANIFEST).expect("manifest is valid json");

        assert_eq!(manifest["schemaVersion"], 1);
        assert_eq!(manifest["id"], DesktopTarget::Omarchy.plugin_id());
        assert!(manifest["name"].is_string());
        assert!(manifest["version"].is_string());
        assert_eq!(
            manifest["kinds"],
            serde_json::json!(["bar-widget"]),
            "the shell loads it as a bar widget"
        );

        let entry = manifest["entryPoints"]["barWidget"]
            .as_str()
            .expect("a bar widget names its entry point");
        assert!(
            ASSETS.iter().any(|(name, _)| *name == entry),
            "the entry point {entry} must be a file we install"
        );

        // The shell swaps a plugin declaring `clonedFrom` into the source
        // widget's own bar slot on enable, and swaps the built-in back on
        // disable. Losing this field silently reverts install to "lands in
        // some section" and uninstall to "leaves a hole in the bar".
        assert_eq!(manifest["omarchy"]["clonedFrom"], "omarchy.workspaces");
    }

    #[test]
    fn the_expected_version_comes_from_the_manifest_itself() {
        assert_eq!(expected_version(), manifest_version(MANIFEST).unwrap());
        assert!(!expected_version().is_empty());
    }

    /// The asset with its comments removed. What the widget is forbidden to do
    /// is a property of its code; the prose above that code is free to name
    /// `amon status` while explaining why the widget does not run it.
    fn code_of(qml: &str) -> String {
        qml.lines()
            .map(|line| line.split("//").next().unwrap_or_default())
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn the_widget_only_ever_connects_to_the_daemon() {
        // The bar observes: it must not be able to start a daemon, and it must
        // not poll the CLI either. Guarding the assets because this is a
        // property of what we ship, not of any Rust.
        for (name, qml) in [
            ("AgentStates.qml", AGENT_STATES_QML),
            ("Workspaces.qml", WORKSPACES_QML),
        ] {
            let code = code_of(qml);
            assert!(!code.contains("Process"), "{name} must not spawn anything");
            assert!(
                !code.contains("amon status"),
                "{name} must not poll the CLI"
            );
        }
        assert!(AGENT_STATES_QML.contains("amond.sock"));
    }

    #[test]
    fn the_fork_keeps_upstreams_behaviour() {
        // ADR-0008: the only difference from Omarchy's widget is what a
        // workspace's label says, so the parts that make it Omarchy's widget
        // have to still be there and still be wired up. If this drifts, every
        // re-sync becomes a merge.
        let wired = WORKSPACES_QML
            .lines()
            .any(|line| line.contains("onPressed:") && line.contains("focusWorkspace"));
        assert!(wired, "clicking still calls upstream's focusWorkspace");
        assert!(
            WORKSPACES_QML.contains("hl.dsp.focus({ workspace = "),
            "and it still switches workspace the way upstream does"
        );
        for landmark in [
            "workspaceIds()",
            "Hyprland.focusedWorkspace",
            "toplevels.values.length",
        ] {
            assert!(
                WORKSPACES_QML.contains(landmark),
                "upstream's {landmark} must survive the fork"
            );
        }
    }

    #[test]
    fn the_fork_records_where_it_came_from() {
        // Re-deriving is only possible if the file says what to re-derive from
        // and which lines are ours.
        assert!(WORKSPACES_QML.contains("upstream:"), "provenance recorded");
        assert_eq!(
            WORKSPACES_QML
                .lines()
                .filter(|line| line.contains("// amon:"))
                .count(),
            4,
            "exactly the four changes ADR-0008 allows, each marked"
        );
    }
}
