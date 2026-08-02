//! Desktop integrations — amon's own, beside the vendored agent installer.
//!
//! An agent Integration writes a hook into an agent's config so the agent
//! reports to amon. A desktop Integration is the other direction: an artifact
//! that *subscribes*, showing amon's state where the user already looks. Both
//! are installed by `amon install <target>` and both report whether the copy on
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

pub fn parse_target(name: &str) -> Option<DesktopTarget> {
    DesktopTarget::ALL
        .into_iter()
        .find(|target| target.label().eq_ignore_ascii_case(name))
}

const MANIFEST: &str = include_str!("assets/omarchy/manifest.json");
const WORKSPACES_QML: &str = include_str!("assets/omarchy/Workspaces.qml");
const AGENT_DOTS_QML: &str = include_str!("assets/omarchy/AgentDots.qml");

const MANIFEST_NAME: &str = "manifest.json";

/// The widget's own files. Written manifest-last and removed manifest-first,
/// because the manifest is what everything else keys off: it is what Omarchy
/// loads the plugin by and what [`status`] reads a version out of, so a
/// half-written plugin must never be one that carries a current manifest.
const ASSETS: [(&str, &str); 2] = [
    ("Workspaces.qml", WORKSPACES_QML),
    ("AgentDots.qml", AGENT_DOTS_QML),
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

    let manifest = directory.join(MANIFEST_NAME);
    match std::fs::symlink_metadata(&manifest) {
        // A directory with no manifest of ours in it: only ours if it is empty,
        // so a fresh install can still use a directory someone precreated.
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            Ok(std::fs::read_dir(directory)?.next().is_none())
        }
        Err(error) => Err(error),
        Ok(metadata) if metadata.file_type().is_symlink() => Ok(false),
        Ok(_) => {
            let installed = std::fs::read_to_string(&manifest)?;
            Ok(manifest_id(&installed).as_deref() == manifest_id(MANIFEST).as_deref())
        }
    }
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

/// Writes the plugin, returning the notes to print — including the two Omarchy
/// commands the user runs to enable it.
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
        "Enable it, then put it in the bar in place of the built-in workspaces:".to_string(),
        format!("  omarchy plugin enable {id}"),
        format!("  omarchy bar plugin add {id}"),
        "  omarchy bar plugin remove omarchy.workspaces".to_string(),
    ])
}

/// Removes exactly what [`install`] wrote.
pub fn uninstall(target: DesktopTarget) -> io::Result<Vec<String>> {
    let directory = plugin_dir(target).ok_or_else(missing_home)?;
    if std::fs::symlink_metadata(&directory).is_err() {
        return Ok(vec![format!("{} is not installed", target.plugin_id())]);
    }
    if !ours(&directory)? {
        return Err(not_ours(&directory));
    }

    // Manifest first, so an interrupted removal leaves something the shell no
    // longer loads rather than a plugin missing its widget.
    for name in std::iter::once(MANIFEST_NAME).chain(ASSETS.iter().map(|(name, _)| *name)) {
        match std::fs::remove_file(directory.join(name)) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    // Plain `remove_dir`: it fails when anything else is in there, which is
    // exactly the outcome wanted — whatever that is, it is not amon's.
    let _ = std::fs::remove_dir(&directory);

    let id = target.plugin_id();
    Ok(vec![
        format!("removed {} from {}", id, directory.display()),
        format!("  omarchy bar plugin remove {id}"),
    ])
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
        Some(version) if *version == expected && assets_match(&directory) => InstallState::Current,
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

/// Whether every file on disk is byte-for-byte the one this build ships.
fn assets_match(directory: &Path) -> bool {
    ASSETS.iter().all(|(name, contents)| {
        std::fs::read_to_string(directory.join(name)).is_ok_and(|found| found == *contents)
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
    }

    #[test]
    fn the_expected_version_comes_from_the_manifest_itself() {
        assert_eq!(expected_version(), manifest_version(MANIFEST).unwrap());
        assert!(!expected_version().is_empty());
    }

    #[test]
    fn the_widget_only_ever_connects_to_the_daemon() {
        // The bar observes; it must not be able to start a daemon. Guarding the
        // asset because this is a property of what we ship, not of any Rust.
        assert!(
            !AGENT_DOTS_QML.contains("Process") && !WORKSPACES_QML.contains("amon status"),
            "the widget must not spawn anything"
        );
        assert!(AGENT_DOTS_QML.contains("amond.sock"));
    }

    #[test]
    fn the_fork_keeps_upstreams_behaviour() {
        // ADR-0008: the only difference from Omarchy's widget is the dot, so
        // the parts that make it Omarchy's widget have to still be there and
        // still be wired up. If this drifts, every re-sync becomes a merge.
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
            3,
            "exactly the three additions ADR-0008 allows, each marked"
        );
    }
}
