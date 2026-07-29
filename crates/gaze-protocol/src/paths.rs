//! Where the sockets live.
//!
//! Part of the protocol rather than of either end: the daemon binds what
//! clients connect to, and a wrapper's hook socket is the address it hands its
//! agent in `GAZE_SOCKET_PATH`.

use std::path::{Path, PathBuf};

/// Unix socket paths are copied into a fixed-size field in `sockaddr_un`,
/// which is 108 bytes on Linux and 104 on macOS. Bind fails outright past
/// that, so gaze checks rather than discovers it at runtime.
const MAX_SOCKET_PATH: usize = 100;

/// Runtime directory for gaze's sockets. `XDG_RUNTIME_DIR` when set — it is
/// per-user, on tmpfs, and cleaned up at logout — otherwise a per-uid
/// directory under the temp dir, which we create with 0700 so another user
/// cannot plant a socket there.
pub fn runtime_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("XDG_RUNTIME_DIR").filter(|dir| !dir.is_empty()) {
        return PathBuf::from(dir).join("gaze");
    }
    short_runtime_dir()
}

/// The fallback: as short as a per-user directory can reasonably be.
fn short_runtime_dir() -> PathBuf {
    let uid = unsafe { libc::getuid() };
    std::env::temp_dir().join(format!("gaze-{uid}"))
}

/// Falls back to the short directory when a socket under `runtime_dir` would
/// not fit. An unusually deep `XDG_RUNTIME_DIR` or `TMPDIR` is rare but makes
/// every socket unbindable, which is a confusing way to fail.
fn socket_in_runtime_dir(relative: &Path) -> PathBuf {
    let preferred = runtime_dir().join(relative);
    if preferred.as_os_str().len() <= MAX_SOCKET_PATH {
        return preferred;
    }
    short_runtime_dir().join(relative)
}

/// The daemon's socket. Also the rendezvous that decides who *is* the daemon:
/// whoever binds it first wins, so racing wrappers cannot both become one.
pub fn daemon_socket() -> PathBuf {
    if let Some(path) = std::env::var_os("GAZE_DAEMON_SOCKET").filter(|path| !path.is_empty()) {
        return PathBuf::from(path);
    }
    socket_in_runtime_dir(Path::new("gazed.sock"))
}

/// A wrapper's own socket, where its agent's hooks report.
pub fn agent_socket(agent_id: &str) -> PathBuf {
    socket_in_runtime_dir(&PathBuf::from("agents").join(format!("{agent_id}.sock")))
}

/// Creates a directory only the current user can enter.
pub fn ensure_private_dir(dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}
