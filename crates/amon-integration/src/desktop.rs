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

/// Every file the plugin directory is made of. Uninstall removes exactly these
/// and then the directory, so nothing amon did not write is ever deleted.
const FILES: [(&str, &str); 3] = [
    ("manifest.json", MANIFEST),
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

/// `$XDG_CONFIG_HOME/omarchy`, else `~/.config/omarchy`.
fn omarchy_config_dir() -> Option<PathBuf> {
    if let Some(config) = std::env::var_os("XDG_CONFIG_HOME").filter(|dir| !dir.is_empty()) {
        return Some(PathBuf::from(config).join("omarchy"));
    }
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
    io::Error::other("cannot find a config directory: neither XDG_CONFIG_HOME nor HOME is set")
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
    std::fs::create_dir_all(&directory)?;
    for (name, contents) in FILES {
        std::fs::write(directory.join(name), contents)?;
    }

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
    if !directory.exists() {
        return Ok(vec![format!("{} is not installed", target.plugin_id())]);
    }

    for (name, _) in FILES {
        match std::fs::remove_file(directory.join(name)) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    // Only if amon's files were all it held: anything else in there is the
    // user's, and takes the directory with it.
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
        Some(version) if *version == expected => InstallState::Current,
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
    let manifest = std::fs::read_to_string(directory.join("manifest.json")).ok()?;
    manifest_version(&manifest)
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
            FILES.iter().any(|(name, _)| *name == entry),
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
    fn the_fork_keeps_upstreams_click_behaviour() {
        // ADR-0008: the only difference from Omarchy's widget is the dot. If
        // this drifts, every re-sync becomes a merge.
        assert!(WORKSPACES_QML.contains("hl.dsp.focus({ workspace = "));
    }
}
