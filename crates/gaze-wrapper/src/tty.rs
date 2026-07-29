//! The real terminal: raw mode and window size.
//!
//! Gaze must be invisible to the agent, which means the user's terminal has to
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
    /// Returns `Ok(None)` when stdin is not a terminal — gaze is being piped,
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

/// Installs a SIGWINCH handler that just sets a flag. Signal handlers may only
/// touch async-signal-safe state, so the actual resize happens on the main
/// loop when it notices.
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
