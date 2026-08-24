//! Wrapping the agents Omarchy launches, which no alias can reach.
//!
//! `amon setup` aliases an agent's name in the shell rc (ADR-0009), and that
//! covers agents you start by typing their name. Omarchy's launcher starts one
//! for you through `xdg-terminal-exec -e`, which execs the binary directly —
//! no shell, so no alias. A directory of tiny executables named after the same
//! agents, early on the session `PATH`, closes that gap (ADR-0013).
//!
//! The set is the set the aliases already cover: exactly the agents `amon
//! setup` wrapped, from the same [`crate::command_names`]. An agent you never
//! set up runs bare, the way it did before amon existed.

use std::io;
use std::path::PathBuf;

use crate::api::schema::IntegrationTarget;

fn home() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .filter(|home| !home.is_empty())
        .map(PathBuf::from)
}

fn missing_home() -> io::Error {
    io::Error::other("cannot find the home directory: HOME is not set")
}

/// Where the shims live.
///
/// Under `~/.local/share` because they are generated executables rather than
/// configuration: nobody edits them, and `amon setup` rewrites them whole.
/// `amon-dev` in a debug build, the same split every other amon directory
/// makes, so a development build cannot shadow agents for the real session.
pub fn dir() -> Option<PathBuf> {
    home().map(|home| {
        home.join(".local/share")
            .join(amon_protocol::paths::app_dir_name())
            .join("shims")
    })
}

/// One shim: run the agent under amon, unless amon is already running it.
///
/// Two guards, and both are needed for different reasons.
///
/// Stripping this directory from `PATH` is what stops the shim calling itself:
/// `amon claude` spawns `claude` through a `PATH` lookup, which would find this
/// file again and recurse forever. Once stripped, the lookup reaches the real
/// agent.
///
/// A third check keeps an amon that has been deleted without `amon remove`
/// from taking the agent down with it: a shim that cannot find the binary runs
/// the agent bare. Losing the wrapping is a far smaller failure than an agent
/// that will not start, and it is the same guard `amon.lua` makes before it
/// rebinds the workspace keys.
///
/// The `AMON_AGENT_ID` check is what stops it wrapping twice. Stripping alone
/// terminates the loop but still puts two amon processes around one agent when
/// amon was invoked directly — `amon claude` typed in a terminal whose `PATH`
/// carries this directory. That variable is set by the wrapper for its own
/// child, so seeing it means "amon is already here": run the agent itself.
fn shim(command: &str, directory: &str, amon: &str) -> String {
    format!(
        r#"#!/bin/bash
# Managed by amon. `amon setup` rewrites this file; `amon remove` deletes it.
#
# Omarchy launches agents through a terminal's -e, which execs the binary
# directly — no shell, so amon's alias never fires. This stands in for the
# agent on PATH so that launch runs under amon too (ADR-0013).

# Without this, the lookup below finds this file again and recurses forever.
#
# The directory goes through a variable rather than into the substitution
# literally: bash splits ${{var//pattern/replacement}} at the first slash it
# parses, and a path is full of them — inlining one silently rewrites PATH into
# nonsense. Expansion happens after that split, so a variable is safe.
dir={directory}
PATH=":$PATH:"
PATH="${{PATH//:$dir:/:}}"
PATH="${{PATH#:}}"
PATH="${{PATH%:}}"
export PATH

# Already inside amon: the wrapper sets this for its own child, so another
# layer would register the same agent twice.
if [[ -n ${{AMON_AGENT_ID:-}} ]]; then
  exec {command} "$@"
fi

# An amon deleted without `amon remove` must not take the agent with it. PATH
# has already lost this directory, so this reaches the real one — losing the
# wrapping is a far smaller failure than an agent that will not start.
if [[ ! -x {amon} ]]; then
  exec {command} "$@"
fi

exec {amon} {command} "$@"
"#
    )
}

/// The command names this target answers to, which are the shims it owns.
fn commands(target: IntegrationTarget) -> &'static [&'static str] {
    crate::command_names(target)
}

/// Writes this target's shims, replacing any that are there.
pub fn install(target: IntegrationTarget) -> io::Result<()> {
    let directory = dir().ok_or_else(missing_home)?;
    std::fs::create_dir_all(&directory)?;
    let amon = std::env::current_exe()?;
    let (Some(directory_text), Some(amon_text)) = (directory.to_str(), amon.to_str()) else {
        return Err(io::Error::other(
            "amon's own paths are not valid UTF-8, so a shim cannot be written",
        ));
    };

    for command in commands(target) {
        let path = directory.join(command);
        std::fs::write(
            &path,
            shim(command, &quoted(directory_text), &quoted(amon_text)),
        )?;
        make_executable(&path)?;
    }
    Ok(())
}

/// Removes this target's shims, and the directory once the last one goes.
///
/// Deleting is the whole of it. Unlike the shell rc, this is a file amon wrote
/// in a directory amon owns: there is no symlink to refuse and no fence to
/// parse, so removal either works or is a real error worth reporting.
pub fn uninstall(target: IntegrationTarget) -> io::Result<()> {
    let Some(directory) = dir() else {
        return Ok(());
    };
    for command in commands(target) {
        match std::fs::remove_file(directory.join(command)) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    prune(&directory);
    Ok(())
}

/// Takes the whole directory, orphans included.
///
/// `amon remove` sweeps rather than removing target by target, for the reason
/// the alias block is swept: a shim whose integration has already gone keeps
/// taking over its agent's name, and removing only what is currently installed
/// would leave exactly those behind.
pub fn remove_all() -> io::Result<Vec<String>> {
    let Some(directory) = dir() else {
        return Ok(Vec::new());
    };
    let taken = installed();
    if taken.is_empty() && !directory.exists() {
        return Ok(Vec::new());
    }
    match std::fs::remove_dir_all(&directory) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    if taken.is_empty() {
        return Ok(Vec::new());
    }
    Ok(vec![format!(
        "{} no longer run under amon when Omarchy launches them",
        taken.join(", ")
    )])
}

/// Every shim on disk, whether or not its integration is still installed.
pub fn installed() -> Vec<String> {
    let Some(directory) = dir() else {
        return Vec::new();
    };
    let Ok(entries) = std::fs::read_dir(&directory) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .flatten()
        .filter(|entry| entry.path().is_file())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

/// Whether this target's shims are all present.
pub fn is_installed(target: IntegrationTarget) -> bool {
    let Some(directory) = dir() else {
        return false;
    };
    commands(target)
        .iter()
        .all(|command| directory.join(command).is_file())
}

/// Shims whose integration is gone — the ones a per-target removal cannot
/// reach, because there is no target left to name.
pub fn stranded() -> Vec<String> {
    let live: Vec<&str> = crate::statuses()
        .into_iter()
        .filter(|status| status.state != crate::InstallState::NotInstalled)
        .flat_map(|status| commands(status.target).iter().copied())
        .collect();
    installed()
        .into_iter()
        .filter(|name| !live.contains(&name.as_str()))
        .collect()
}

/// Removes the directory when nothing is left in it, so a `PATH` entry never
/// outlives the last shim by more than a session.
fn prune(directory: &std::path::Path) {
    if installed().is_empty() {
        let _ = std::fs::remove_dir(directory);
    }
}

/// Wraps a path so the shell treats it as one word whatever is in it.
fn quoted(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn make_executable(path: &std::path::Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut permissions = std::fs::metadata(path)?.permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(path, permissions)
}
