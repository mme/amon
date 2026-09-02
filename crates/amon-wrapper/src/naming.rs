//! Wearing the agent's name inside a runtime pane (ADR-0022).
//!
//! herdr and luvus identify the agent in a pane by the process's *name* —
//! never its executable path. When amon wraps, the pane's process is amon, so
//! the runtime would see `amon` and show nothing. Inside a runtime, amon
//! therefore presents the agent's own command name in the fields the runtimes
//! read: the kernel `comm` (herdr) and the process title (luvus).
//!
//! This is scoped to runtime panes. Everywhere else amon is plainly `amon`.

/// Present `name` as this process's `comm` — the short name herdr reads from
/// `/proc/<pid>/comm`. Best-effort: a failure means the runtime may not detect
/// the agent, which is no worse than stepping aside used to be, so it never
/// aborts the wrap.
///
/// `comm` is capped at 15 bytes by the kernel; every agent command name we
/// ship fits, and a longer one is truncated rather than rejected.
pub fn wear_comm(name: &str) {
    let Ok(cstr) = std::ffi::CString::new(name) else {
        return;
    };
    // SAFETY: PR_SET_NAME copies up to 16 bytes from the pointer into the
    // kernel's task comm; `cstr` is a valid NUL-terminated buffer that
    // outlives the call.
    unsafe {
        libc::prctl(libc::PR_SET_NAME, cstr.as_ptr() as libc::c_ulong, 0, 0, 0);
    }
}

/// The command name a runtime will match, for the agent amon is wrapping.
///
/// It is the agent's own label (`claude`, `codex`, …) — the exact string the
/// vendored identification matches — taken from the parsed agent when amon
/// recognised one, and from the argv's own basename otherwise so an unknown
/// agent at least presents its real command rather than `amon`.
pub fn agent_command_name(
    agent: Option<amon_detect::Agent>,
    argv0: &std::ffi::OsStr,
) -> Option<String> {
    if let Some(agent) = agent {
        return Some(amon_detect::agent_label(agent).to_string());
    }
    std::path::Path::new(argv0)
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .filter(|name| !name.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsStr;

    #[test]
    fn a_known_agent_wears_its_label() {
        let agent = amon_detect::identify_agent("claude");
        assert_eq!(
            agent_command_name(agent, OsStr::new("/usr/bin/amon")).as_deref(),
            Some("claude")
        );
    }

    #[test]
    fn an_unknown_agent_wears_its_own_basename_not_amon() {
        assert_eq!(
            agent_command_name(None, OsStr::new("/opt/tools/weirdcli")).as_deref(),
            Some("weirdcli")
        );
    }

    #[test]
    fn wearing_comm_takes_effect() {
        // comm is per-thread, so read it back through the kernel for the same
        // thread that set it rather than /proc/self/comm (the main thread).
        wear_comm("claude-x");
        let mut buf = [0u8; 16];
        // SAFETY: PR_GET_NAME writes up to 16 bytes into `buf`.
        unsafe {
            libc::prctl(
                libc::PR_GET_NAME,
                buf.as_mut_ptr() as libc::c_ulong,
                0,
                0,
                0,
            );
        }
        let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
        assert_eq!(std::str::from_utf8(&buf[..end]).unwrap(), "claude-x");
    }
}
