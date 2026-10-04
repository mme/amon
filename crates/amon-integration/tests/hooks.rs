//! Installing and removing the hook integrations.
//!
//! What an installed hook sends when it runs is checked in amon-cli's
//! `tests/hooks.rs`: the scripts hand their JSON to `amon hook input`, so they
//! need the amon binary, which only that crate's tests can name.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};

/// Somewhere short enough for a unix socket path, unique per test.
fn scratch(name: &str) -> PathBuf {
    static NEXT: AtomicU32 = AtomicU32::new(0);
    let dir = std::env::temp_dir().join(format!(
        "wlh{}-{}-{name}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("scratch dir");
    dir
}

/// Installs an agent's integration into a throwaway config directory and
/// returns the hook script's path.
fn install_into(
    target: amon_integration::IntegrationTarget,
    config_env: &str,
    dir: &Path,
) -> Vec<String> {
    // SAFETY: single-threaded test process; nextest gives each test its own.
    unsafe { std::env::set_var(config_env, dir) };
    amon_integration::install(target).expect("install succeeds")
}

#[test]
fn hooks_stay_silent_unless_amon_is_wrapping_the_agent() {
    // Installed hooks run on every agent session, including ones amon knows
    // nothing about. Without the environment they must do nothing at all.
    let config = scratch("quiet");
    install_into(
        amon_integration::IntegrationTarget::Claude,
        "CLAUDE_CONFIG_DIR",
        &config,
    );
    let hook = config.join("hooks/amon-agent-state.sh");

    let output = Command::new("sh")
        .arg(&hook)
        .arg("session")
        .env_remove(amon_protocol::env::AMON_ENV)
        .env_remove(amon_protocol::env::SOCKET_PATH)
        .env_remove(amon_protocol::env::AGENT_ID)
        .stdin(Stdio::null())
        .output()
        .expect("hook runs");

    assert!(output.status.success(), "a hook must never fail its agent");
    assert!(
        output.stdout.is_empty() && output.stderr.is_empty(),
        "a hook must never print into the agent's session: {output:?}"
    );

    let _ = std::fs::remove_dir_all(&config);
}

#[test]
fn installing_twice_is_idempotent() {
    let config = scratch("twice");
    install_into(
        amon_integration::IntegrationTarget::Claude,
        "CLAUDE_CONFIG_DIR",
        &config,
    );
    let settings = config.join("settings.json");
    let after_first = std::fs::read_to_string(&settings).expect("settings written");

    amon_integration::install(amon_integration::IntegrationTarget::Claude).expect("reinstall");
    let after_second = std::fs::read_to_string(&settings).expect("settings still there");

    assert_eq!(
        after_first, after_second,
        "reinstalling must not duplicate the hook registration"
    );

    let _ = std::fs::remove_dir_all(&config);
}

#[test]
fn uninstall_removes_what_install_added() {
    let config = scratch("uninstall");
    install_into(
        amon_integration::IntegrationTarget::Claude,
        "CLAUDE_CONFIG_DIR",
        &config,
    );
    let hook = config.join("hooks/amon-agent-state.sh");
    assert!(hook.exists());

    amon_integration::uninstall(amon_integration::IntegrationTarget::Claude).expect("uninstall");

    assert!(!hook.exists(), "the managed hook file is gone");
    let settings = std::fs::read_to_string(config.join("settings.json")).unwrap_or_default();
    assert!(
        !settings.contains("amon-agent-state"),
        "and its registration: {settings}"
    );

    let _ = std::fs::remove_dir_all(&config);
}

#[test]
fn the_pi_extension_installs_and_uninstalls() {
    // pi's integration is a TypeScript extension rather than a shell hook, so
    // there is nothing to execute here — what matters is that it lands in the
    // extension directory under amon's name and leaves cleanly.
    let config = scratch("pi");
    install_into(
        amon_integration::IntegrationTarget::Pi,
        "PI_CODING_AGENT_DIR",
        &config,
    );

    let extension = config.join("extensions/amon-agent-state.ts");
    assert!(extension.exists(), "installed at {}", extension.display());
    let source = std::fs::read_to_string(&extension).expect("readable");
    assert!(
        source.contains("AMON_SOCKET_PATH") && source.contains("agent.report_state"),
        "the extension speaks amon's protocol"
    );

    amon_integration::uninstall(amon_integration::IntegrationTarget::Pi).expect("uninstall");
    assert!(!extension.exists(), "and is removed again");

    let _ = std::fs::remove_dir_all(&config);
}

#[test]
fn every_shipped_hook_starts_with_its_shebang() {
    // The agents execute these files directly, so a shebang anywhere but the
    // first line makes the hook unrunnable. The provenance header goes after
    // it, which is easy to get wrong when re-vendoring.
    let assets = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/vendor/integration/assets");
    let mut checked = 0;

    for entry in walk(&assets) {
        let Ok(contents) = std::fs::read_to_string(&entry) else {
            continue;
        };
        if !contents.contains("#!/") {
            continue;
        }
        checked += 1;
        assert!(
            contents.starts_with("#!"),
            "{} has a shebang that is not on line 1",
            entry.display()
        );
    }

    assert!(checked > 0, "found no executable hook assets to check");
}

fn walk(dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return files;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            files.extend(walk(&path));
        } else {
            files.push(path);
        }
    }
    files
}
