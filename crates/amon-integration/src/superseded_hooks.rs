//! amon's own state hooks in place of herdr's python3 ones (ADR-0020: a
//! supersession; issue #77).
//!
//! herdr's installer is kept whole - it is the part that edits each agent's
//! config without disturbing anything else (ADR-0004). Only the script it
//! writes is replaced: right after it lands, amon writes its own over it, a
//! few lines of shell that hand the agent's JSON to `amon hook input`, where
//! `hook_input.rs` applies the rules herdr's Python did. herdr's markers stay,
//! so the vendored status check reads the file exactly as before; the vendored
//! scripts stay in the tree, under `scripts/revendor.sh`'s drift watch, as
//! the reference those rules follow.

use std::io;
use std::path::Path;

use crate::api::schema::IntegrationTarget;
use crate::integration;

const TEMPLATE: &str = include_str!("assets/agent-hook.sh");

/// The line that tells amon's script from herdr's.
const MARKER: &str = "# AMON_HOOK_INPUT=1";

/// The hook `amon hook input` reads for a target whose installed script is a
/// shell script amon supersedes; `None` for the rest (the plugins and
/// extensions, which run inside their agent and need no interpreter).
pub(crate) fn hook_name(target: IntegrationTarget) -> Option<&'static str> {
    if cfg!(windows) {
        return None;
    }
    Some(match target {
        IntegrationTarget::Claude => "claude",
        IntegrationTarget::Codex => "codex",
        IntegrationTarget::Copilot => "copilot",
        IntegrationTarget::Cursor => "cursor",
        IntegrationTarget::Devin => "devin",
        IntegrationTarget::Droid => "droid",
        IntegrationTarget::Grok => "grok",
        IntegrationTarget::Kimi => "kimi",
        IntegrationTarget::Mastracode => "mastracode",
        IntegrationTarget::Qodercli => "qodercli",
        IntegrationTarget::Qwen => "qwen",
        IntegrationTarget::AntigravityCli => "antigravity_cli",
        _ => return None,
    })
}

/// Where herdr's installer puts this target's script.
fn installed_path(target: IntegrationTarget) -> Option<std::path::PathBuf> {
    integration::installed_integration_statuses()
        .into_iter()
        .find(|status| status.target == target)
        .map(|status| status.path)
}

/// Replaces the script herdr's installer just wrote. A target amon does not
/// supersede is left alone.
pub(crate) fn write_over(target: IntegrationTarget) -> io::Result<()> {
    let Some(hook) = hook_name(target) else {
        return Ok(());
    };
    let Some(path) = installed_path(target) else {
        return Ok(());
    };
    let written = std::fs::read_to_string(&path)?;
    let script = render(&written, hook).ok_or_else(|| {
        io::Error::other(format!("{} carries no integration markers", path.display()))
    })?;
    std::fs::write(&path, script)
}

/// amon's script for `hook`, with the markers of herdr's `written` one.
fn render(written: &str, hook: &str) -> Option<String> {
    let marker = |name: &str| {
        written.lines().find_map(|line| {
            line.trim()
                .strip_prefix('#')
                .map(str::trim)
                .and_then(|rest| rest.strip_prefix(name))
                .and_then(|rest| rest.strip_prefix('='))
                .map(|value| value.trim().to_owned())
        })
    };
    let id = marker("AMON_INTEGRATION_ID")?;
    let version = marker("AMON_INTEGRATION_VERSION")?;
    // Antigravity reads a JSON object from its hook's stdout on every path.
    let fallback = if hook == "antigravity_cli" {
        "printf '{}\\n'\n"
    } else {
        ""
    };
    Some(
        TEMPLATE
            .replace("__ID__", &id)
            .replace("__VERSION__", &version)
            .replace("__HOOK__", hook)
            .replace("__FALLBACK__", fallback),
    )
}

/// Whether the script at `path` is still herdr's python3 one, for a target
/// amon supersedes - an install from before amon read hooks itself, which
/// the vendored check, seeing current markers, would call current.
pub(crate) fn still_python(target: IntegrationTarget, path: &Path) -> bool {
    hook_name(target).is_some()
        && std::fs::read_to_string(path)
            .is_ok_and(|script| !script.lines().any(|line| line.trim() == MARKER))
}

#[cfg(test)]
mod tests {
    use super::*;

    const HERDR: &str = "#!/bin/sh\n# AMON_INTEGRATION_ID=claude\n# AMON_INTEGRATION_VERSION=9\npython3 - <<'PY'\nPY\n";

    #[test]
    fn the_script_keeps_herdrs_markers_and_calls_amon() {
        let script = render(HERDR, "claude").expect("markers");
        assert!(script.starts_with("#!/bin/sh\n"));
        assert!(script.contains("\n# AMON_INTEGRATION_ID=claude\n"));
        assert!(script.contains("\n# AMON_INTEGRATION_VERSION=9\n"));
        assert!(script.contains("hook input claude \"$@\""));
        assert!(script.lines().any(|line| line == MARKER));
        assert!(!script.contains("python"), "no interpreter: {script}");
        assert!(!script.contains("__"), "every placeholder filled: {script}");
        assert!(!script.contains("printf"));
    }

    #[test]
    fn antigravity_always_answers_with_an_object() {
        let herdr = HERDR.replace("=claude", "=antigravity_cli");
        let script = render(&herdr, "antigravity_cli").expect("markers");
        assert!(script.contains("printf '{}\\n'\nexit 0\n"), "{script}");
    }

    #[test]
    fn a_script_without_markers_is_refused() {
        assert!(render("#!/bin/sh\necho hi\n", "claude").is_none());
    }

    #[test]
    fn every_hook_amon_supersedes_is_one_amon_reads() {
        for target in IntegrationTarget::ALL {
            if let Some(hook) = hook_name(target) {
                assert!(crate::hook_input::HOOKS.contains(&hook), "{hook}");
            }
        }
    }
}
