//! Stands in for herdr's `crate::noninteractive_process`.
//!
//! The manifest updater shells out to curl rather than linking an HTTP stack.
//! gaze keeps that: it costs no dependencies and the catalog fetch is a
//! once-a-day background call.

use std::ffi::OsStr;
use std::process::Command;

/// A subprocess whose stdio the caller controls and which never pops up a
/// console window on Windows.
pub fn command(program: impl AsRef<OsStr>) -> Command {
    let mut command = Command::new(program);
    configure_background_command(&mut command);
    command
}

pub fn curl_command() -> Command {
    command("curl")
}

#[cfg(windows)]
fn configure_background_command(command: &mut Command) {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    command.creation_flags(CREATE_NO_WINDOW);
}

#[cfg(not(windows))]
fn configure_background_command(_command: &mut Command) {}
