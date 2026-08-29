//! The real terminal: raw mode and window size.
//!
//! Amon must be invisible to the agent, which means the user's terminal has to
//! behave exactly as it would without us — every keystroke delivered
//! unprocessed, and the agent's PTY always the same size as the window.

use std::io;
use std::os::fd::{AsRawFd, RawFd};

/// Restores the terminal's original settings when dropped, including on panic.
/// Leaving a terminal in raw mode makes a shell look broken, so this must
/// outlive everything that could fail.
pub struct RawMode {
    fd: RawFd,
    original: libc::termios,
}

impl RawMode {
    /// Returns `Ok(None)` when stdin is not a terminal — amon is being piped,
    /// so there is no raw mode to enter and nothing to restore.
    pub fn enter() -> io::Result<Option<Self>> {
        let fd = io::stdin().as_raw_fd();
        if unsafe { libc::isatty(fd) } != 1 {
            return Ok(None);
        }

        let mut original: libc::termios = unsafe { std::mem::zeroed() };
        if unsafe { libc::tcgetattr(fd, &mut original) } != 0 {
            return Err(io::Error::last_os_error());
        }

        let mut raw = original;
        unsafe { libc::cfmakeraw(&mut raw) };
        if unsafe { libc::tcsetattr(fd, libc::TCSANOW, &raw) } != 0 {
            return Err(io::Error::last_os_error());
        }

        Ok(Some(Self { fd, original }))
    }
}

impl Drop for RawMode {
    fn drop(&mut self) {
        unsafe { libc::tcsetattr(self.fd, libc::TCSANOW, &self.original) };
    }
}

/// Whether stdout is the terminal itself rather than a file or a pipe.
///
/// Asked before amon writes anything of its own: with `amon agent > log`,
/// stdin is still a terminal but stdout is a file, and escape sequences do not
/// belong in someone's log.
pub fn stdout_is_terminal() -> bool {
    unsafe { libc::isatty(io::stdout().as_raw_fd()) == 1 }
}

/// Whether amon has a terminal on either side of it.
///
/// Either side, not both: `amon agent > log` keeps a terminal on stdin and
/// `echo prompt | amon agent` keeps one on stdout, and in both a person is
/// sitting there watching. Neither side is the case that matters — an agent
/// started by another agent takes stdin from `/dev/null` and writes stdout to
/// a file — and that is an agent with no window to jump to (ADR-0016).
///
/// The two fds, not `/dev/tty`. This asks what amon was handed, which is the
/// question; a controlling terminal is inherited by everything a
/// terminal-borne process starts, so an agent launched in the background by a
/// parent that did not `setsid` would still have one and would look
/// interactive when it is not.
///
/// Load-bearing assumption: the agent that starts another agent hands it pipes
/// rather than a PTY. Harnesses sometimes allocate a PTY to coax colour out of
/// a subprocess, and one that did would look interactive here. That costs the
/// suppression, not correctness — the row comes back and amon behaves as it
/// did before this check existed.
pub fn attached_to_terminal() -> bool {
    unsafe {
        libc::isatty(io::stdin().as_raw_fd()) == 1 || libc::isatty(io::stdout().as_raw_fd()) == 1
    }
}

/// The terminal's current size, or a sane default when it cannot be read (a
/// pipe, or a terminal being torn down).
pub fn window_size() -> (u16, u16) {
    let mut size: libc::winsize = unsafe { std::mem::zeroed() };
    let ok = unsafe { libc::ioctl(io::stdout().as_raw_fd(), libc::TIOCGWINSZ, &mut size) } == 0;
    if !ok || size.ws_col == 0 || size.ws_row == 0 {
        return (80, 24);
    }
    (size.ws_col, size.ws_row)
}

/// Forwards signals aimed at amon to the agent instead. Someone killing the
/// visible pid means "stop the agent"; without this, amon would die first
/// and the agent would see a collapsing PTY (SIGHUP) rather than the signal
/// that was actually sent — and amon's cleanup would never run. With it, the
/// agent exits, the wrapper unwinds normally, and the caller sees amon die
/// of the same signal (via the re-raise in the CLI).
pub mod forward {
    use std::sync::atomic::{AtomicI32, Ordering};

    static TARGET: AtomicI32 = AtomicI32::new(0);

    extern "C" fn handle(signal: libc::c_int) {
        let pid = TARGET.load(Ordering::Relaxed);
        if pid > 0 {
            // kill(2) is async-signal-safe; nothing else happens here.
            unsafe { libc::kill(pid, signal) };
        }
    }

    /// Starts forwarding SIGTERM, SIGINT, and SIGHUP to `pid`.
    pub fn arm(pid: u32) {
        TARGET.store(pid as i32, Ordering::Relaxed);
        unsafe {
            let mut action: libc::sigaction = std::mem::zeroed();
            action.sa_sigaction = handle as extern "C" fn(libc::c_int) as usize;
            libc::sigemptyset(&mut action.sa_mask);
            // SA_RESTART: the output reader must keep draining the PTY.
            action.sa_flags = libc::SA_RESTART;
            for signal in [libc::SIGTERM, libc::SIGINT, libc::SIGHUP] {
                libc::sigaction(signal, &action, std::ptr::null_mut());
            }
        }
    }

    /// Stops forwarding — the agent is reaped, and its pid may be reused.
    pub fn disarm() {
        TARGET.store(0, Ordering::Relaxed);
    }
}

/// Installs a SIGWINCH handler that just sets a flag. Signal handlers may only
/// touch async-signal-safe state, so the actual resize happens on the resize
/// thread that polls this flag.
pub fn watch_resizes() -> &'static std::sync::atomic::AtomicBool {
    use std::sync::atomic::AtomicBool;
    static RESIZED: AtomicBool = AtomicBool::new(false);

    extern "C" fn handle(_signal: libc::c_int) {
        RESIZED.store(true, std::sync::atomic::Ordering::Relaxed);
    }

    unsafe {
        let mut action: libc::sigaction = std::mem::zeroed();
        action.sa_sigaction = handle as extern "C" fn(libc::c_int) as usize;
        libc::sigemptyset(&mut action.sa_mask);
        action.sa_flags = libc::SA_RESTART;
        libc::sigaction(libc::SIGWINCH, &action, std::ptr::null_mut());
    }

    &RESIZED
}
