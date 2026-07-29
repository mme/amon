//! Keeping the detection manifests fresh.
//!
//! The daemon is the only writer: wrappers just read what is on disk when they
//! start. herdr checks once at startup, which suits a program you restart
//! often; gaze's daemon can run for weeks, so it also re-checks daily
//! (ADR-0003). Manifests that need a newer engine than the vendored one are
//! refused by the vendored updater itself.

use std::time::Duration;

const REFRESH_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);

/// Refreshes now and every 24 hours after, on a background thread. Failures
/// are logged and retried at the next tick; the bundled manifests mean a
/// daemon that can never reach the catalog still detects agents correctly.
pub fn refresh_periodically() {
    std::thread::spawn(|| loop {
        match gaze_detect::manifests::check_and_update() {
            Ok(updated) if updated > 0 => {
                tracing::info!(updated, "refreshed agent detection manifests");
            }
            Ok(_) => {}
            Err(error) => tracing::warn!("agent detection manifest update failed: {error}"),
        }
        std::thread::sleep(REFRESH_INTERVAL);
    });
}
