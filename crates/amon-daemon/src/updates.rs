//! The daily look for a newer release.
//!
//! One HEAD request against the release page's stable `/releases/latest`
//! redirect — the same contract `install.sh` resolves a version from, so the
//! two cannot disagree about what "latest" means. No API, no rate limits,
//! nothing about this machine in the request. Subscribers hear about a find
//! as an `update_available` event; nothing here downloads or installs
//! anything — upgrading stays the user re-running the installer.

use std::time::{Duration, Instant};

const DEFAULT_URL: &str = "https://github.com/mme/amon/releases/latest";

/// At most one request a day, however long the daemon lives.
const CHECK_EVERY: Duration = Duration::from_secs(24 * 60 * 60);

/// How often the loop wakes to notice `[updates] check` changing. Waking is
/// free — the network is only touched when a check is due *and* allowed.
const CONFIG_POLL: Duration = Duration::from_secs(5 * 60);

/// Starts the checker thread, or declines to.
///
/// A debug build is `amon-dev` end to end and never checks on its own: every
/// test sandbox and dev daemon would otherwise reach the real network. The
/// `AMON_UPDATE_URL` override is both the test hook and the dev enabler —
/// pointing the check somewhere is what turns it on.
pub fn check_periodically(
    registry: crate::Registry,
    config: crate::config::Watcher,
    version: String,
) {
    let url = match std::env::var("AMON_UPDATE_URL") {
        Ok(url) if !url.is_empty() => url,
        _ if cfg!(debug_assertions) => return,
        _ => DEFAULT_URL.to_string(),
    };
    std::thread::spawn(move || {
        let mut last_checked: Option<Instant> = None;
        loop {
            let due = last_checked.is_none_or(|at| at.elapsed() >= CHECK_EVERY);
            if due && config.current().updates.check {
                last_checked = Some(Instant::now());
                if let Some(latest) = latest_release(&url) {
                    // Strictly newer only: a from-source build ahead of the
                    // last release must never be told to downgrade, and the
                    // current release is not an update.
                    if crate::is_newer(&latest, &version) {
                        registry.publish_update(version.clone(), latest);
                    }
                }
            }
            std::thread::sleep(CONFIG_POLL);
        }
    });
}

/// The latest released version, bare (`0.2.0`), read off where the redirect
/// lands. `None` on any failure — offline is normal, and a check that cannot
/// answer says nothing rather than guessing.
fn latest_release(url: &str) -> Option<String> {
    let output = std::process::Command::new("curl")
        .args(["-fsSLI", "-o", "/dev/null", "-w", "%{url_effective}", url])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let effective = String::from_utf8(output.stdout).ok()?;
    let tag = effective.trim().rsplit('/').next()?;
    let version = tag.strip_prefix('v')?;
    version
        .starts_with(|c: char| c.is_ascii_digit())
        .then(|| version.to_string())
}
