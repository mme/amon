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
pub fn status() -> InstallState {
    let Some(path) = dropin_path() else {
        return InstallState::NotInstalled;
    };
    match std::fs::read_to_string(&path) {
        Ok(content) if content == DROPIN => InstallState::Current,
        Ok(_) => InstallState::Outdated,
        Err(_) => InstallState::NotInstalled,
    }
}

/// Writes the drop-in and restarts WirePlumber so it takes effect now.
///
/// The restart is what makes the install real — WirePlumber reads its
/// configuration at startup — and it audibly blips whatever is playing,
/// which the returned notes say out loud rather than leaving the user to
/// wonder. A machine where the restart fails still has the file: the
/// config applies on the next login, and the note says that too.
pub fn install() -> io::Result<Vec<String>> {
    let path = dropin_path().ok_or_else(|| io::Error::other("no home directory"))?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, DROPIN)?;
    let mut notes = Vec::new();
    if restart_wireplumber() {
        notes.push("music now ducks while a notification plays".to_string());
    } else {
        notes.push("installed; it takes effect at your next login".to_string());
    }
    Ok(notes)
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
    let mut notes = Vec::new();
    if restart_wireplumber() {
        notes.push("audio is back to stock".to_string());
    } else {
        notes.push("removed; stock audio returns at your next login".to_string());
    }
    Ok(notes)
}

/// True if WirePlumber was restarted. Through systemd because that is how
/// Omarchy runs it; a machine managing it differently just gets the config
/// at next login.
fn restart_wireplumber() -> bool {
    std::process::Command::new("systemctl")
        .args(["--user", "restart", "wireplumber"])
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}
