//! Where the sockets live.
//!
//! Part of the protocol rather than of either end: the daemon binds what
//! clients connect to, and a wrapper's hook socket is the address it hands its
//! agent in `AMON_SOCKET_PATH`.

use std::path::{Path, PathBuf};

/// Unix socket paths are copied into a fixed-size field in `sockaddr_un`,
/// which is 108 bytes on Linux and 104 on macOS. Bind fails outright past
/// that, so amon checks rather than discovers it at runtime.
const MAX_SOCKET_PATH: usize = 100;

/// `amon` in a release build, `amon-dev` in a debug one — the same split
/// `shim::config::app_dir_name` makes for the state and config directories,
/// spelled out again here because this crate depends on no other amon crate
/// and `debug_assertions` is a compiler flag rather than something to import.
///
/// The two have to agree. The state directory was already per-build while the
/// socket was not, and a daemon only yields to a *strictly* newer version, so a
/// debug daemon would hold the one socket and a release client — equal version,
/// therefore no takeover — would go on reading its own empty state directory
/// and silently fall back to the bundled manifests.
pub fn app_dir_name() -> &'static str {
    if cfg!(debug_assertions) {
        "amon-dev"
    } else {
        "amon"
    }
}

/// Runtime directory for amon's sockets. `XDG_RUNTIME_DIR` when set — it is
/// per-user, on tmpfs, and cleaned up at logout — otherwise a per-uid
/// directory under the temp dir, which we create with 0700 so another user
/// cannot plant a socket there.
pub fn runtime_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("XDG_RUNTIME_DIR").filter(|dir| !dir.is_empty()) {
        return PathBuf::from(dir).join(app_dir_name());
    }
    short_runtime_dir()
}

/// The fallback: as short as a per-user directory can reasonably be.
fn short_runtime_dir() -> PathBuf {
    let uid = unsafe { libc::getuid() };
    std::env::temp_dir().join(format!("{}-{uid}", app_dir_name()))
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
    if let Some(path) = std::env::var_os("AMON_DAEMON_SOCKET").filter(|path| !path.is_empty()) {
        return PathBuf::from(path);
    }
    socket_in_runtime_dir(Path::new("amond.sock"))
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
