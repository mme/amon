//! Ducking other audio while a notification plays: one WirePlumber drop-in.
//!
//! The whole integration is a single file amon owns, written into the
//! directory WirePlumber designates for user configuration. It declares two
//! role buses — Multimedia for everything, Notifications above it — and
//! WirePlumber's own role-based policy does the rest: the daemon tags its
//! sounds `media.role=Notification`, the policy ducks the Multimedia bus
//! while one plays and restores it when the stream ends. No amon process is
//! in the loop at runtime, and deleting the file puts the audio stack back
//! to stock, bit for bit.

use std::io;
use std::path::PathBuf;

use crate::api_surface::InstallState;

/// The drop-in, verbatim. An asset rather than a string built here, so the
/// file on disk and the file in the repository can be diffed directly.
const DROPIN: &str = include_str!("assets/wireplumber/50-amon-ducking.conf");

/// Named for ordering among other drop-ins and for legibility in a directory
/// listing: a user wondering what changed their audio finds "amon" in the
/// name and amon's explanation in the file.
const DROPIN_NAME: &str = "50-amon-ducking.conf";

/// Where WirePlumber reads user drop-ins from, resolved the way WirePlumber
/// itself resolves it.
fn dropin_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME")
                .filter(|value| !value.is_empty())
                .map(|home| PathBuf::from(home).join(".config"))
        })?;
    Some(
        base.join("wireplumber/wireplumber.conf.d")
            .join(DROPIN_NAME),
    )
}

/// Whether this machine has an audio stack to duck: WirePlumber on PATH is
/// the whole test. Without it the drop-in would be an inert file waiting for
/// a session manager that never reads it.
pub fn available() -> bool {
    crate::api_surface::on_path("wireplumber")
}

/// Installed, installed-but-stale (an older amon's copy), or absent.
///
/// Only "the file is not there" means not installed. A file that exists but
/// cannot be read — permissions, invalid UTF-8, a disk saying no — is
/// *something* occupying amon's name, and calling it absent would make
/// `amon remove --all` skip it while `amon doctor` insists nothing is
/// wrong. Stale keeps it visible and removable.
pub fn status() -> InstallState {
    let Some(path) = dropin_path() else {
        return InstallState::NotInstalled;
    };
    match std::fs::read_to_string(&path) {
        Ok(content) if content == DROPIN => InstallState::Current,
        Ok(_) => InstallState::Outdated,
        Err(error) if error.kind() == io::ErrorKind::NotFound => InstallState::NotInstalled,
        Err(_) => InstallState::Outdated,
    }
}

/// Writes the drop-in and restarts WirePlumber so it takes effect now.
///
/// The restart is what makes the install real — WirePlumber reads its
/// configuration at startup — and it audibly blips whatever is playing,
/// which the returned notes say out loud rather than leaving the user to
/// wonder.
///
/// Failure handling took the shape it has because the failure is not
/// hypothetical: a config fragment WirePlumber refuses sends it into a
/// crash-loop until systemd gives up, and the user's audio is simply gone.
/// So the write is staged-then-renamed (no torn file can exist), a restart
/// that leaves the service dead rolls the file back and restarts once more
/// before reporting the error, and only a machine with no systemctl at all
/// gets the file without a restart — there the config waits for next login.
pub fn install() -> io::Result<Vec<String>> {
    let path = dropin_path().ok_or_else(|| io::Error::other("no home directory"))?;
    // A symlink here belongs to the user — a dotfiles checkout, usually —
    // and writing through it would edit a file amon does not own. The same
    // line every other integration draws.
    if path
        .symlink_metadata()
        .is_ok_and(|meta| meta.file_type().is_symlink())
    {
        return Err(io::Error::other(format!(
            "{} is a symlink; amon will not write through it",
            path.display()
        )));
    }
    // Someone with their own role-based audio config has already made the
    // choices this file makes, and WirePlumber merges fragments by
    // appending — two sets of role buses under the same names is a broken
    // profile, not a second opinion. Theirs stands.
    if let Some(theirs) = foreign_role_config(&path) {
        return Err(io::Error::other(format!(
            "{} already sets up role-based audio; leaving it alone",
            theirs.display()
        )));
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let staging = path.with_extension("conf.amon-staging");
    std::fs::write(&staging, DROPIN)?;
    std::fs::rename(&staging, &path)?;

    match restart_wireplumber() {
        Restart::Restarted => Ok(vec![
            "music now ducks while a notification plays".to_string()
        ]),
        Restart::NoSystemctl => Ok(vec![
            "installed; it takes effect at your next login".to_string()
        ]),
        Restart::Failed => {
            // The service is down and our file is the prime suspect: take
            // it back out and get audio running again before saying why.
            let _ = std::fs::remove_file(&path);
            let _ = restart_wireplumber();
            Err(io::Error::other(
                "wireplumber refused to restart with the ducking config; \
                 removed it again and restarted without it",
            ))
        }
    }
}

/// Another fragment in the drop-in directory that already declares role
/// buses — anything mentioning the role-loopback machinery that is not
/// amon's own file.
fn foreign_role_config(ours: &std::path::Path) -> Option<PathBuf> {
    let dir = ours.parent()?;
    let entries = std::fs::read_dir(dir).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path == ours {
            continue;
        }
        let Ok(content) = std::fs::read_to_string(&path) else {
            continue;
        };
        if content.contains("loopback.sink.role")
            || content.contains("policy.linking.role-based.loopbacks")
        {
            return Some(path);
        }
    }
    None
}

/// Removes the drop-in and restarts WirePlumber: the audio stack is stock
/// again the moment this returns. A file that is already gone is success —
/// the state asked for is the state there is.
pub fn uninstall() -> io::Result<Vec<String>> {
    let Some(path) = dropin_path() else {
        return Ok(Vec::new());
    };
    match std::fs::remove_file(&path) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    }
    let note = match restart_wireplumber() {
        Restart::Restarted => "audio is back to stock",
        // Removal cannot make the config worse; whatever the restart's
        // trouble is, the next login sees a directory without our file.
        Restart::NoSystemctl | Restart::Failed => "removed; stock audio returns at your next login",
    };
    Ok(vec![note.to_string()])
}

enum Restart {
    Restarted,
    /// No systemctl to ask — a machine running WirePlumber some other way
    /// gets the config change at next login.
    NoSystemctl,
    /// systemctl ran and the service did not come back.
    Failed,
}

/// Restarts WirePlumber, through systemd because that is how Omarchy runs
/// it. "Restarted" means the service is actually *active* a moment later —
/// measured, not assumed: a fragment WirePlumber refuses makes `restart`
/// return success (the process started) and then crash-loop until systemd
/// gives up, so the exit code alone reports exactly the wrong thing in
/// exactly the case that matters.
fn restart_wireplumber() -> Restart {
    // Cleared first: a service that already hit its start limit refuses
    // every restart until someone resets it, and after a rollback this is
    // what lets the recovery restart actually run.
    let _ = std::process::Command::new("systemctl")
        .args(["--user", "reset-failed", "wireplumber"])
        .status();
    let restarted = std::process::Command::new("systemctl")
        .args(["--user", "restart", "wireplumber"])
        .status();
    match restarted {
        Err(_) => return Restart::NoSystemctl,
        Ok(status) if !status.success() => return Restart::Failed,
        Ok(_) => {}
    }
    // Long enough for a config-rejecting crash-loop to burn through its
    // start limit; a healthy start is active well before this.
    std::thread::sleep(std::time::Duration::from_millis(1200));
    let active = std::process::Command::new("systemctl")
        .args(["--user", "is-active", "--quiet", "wireplumber"])
        .status();
    match active {
        Ok(status) if status.success() => Restart::Restarted,
        _ => Restart::Failed,
    }
}
