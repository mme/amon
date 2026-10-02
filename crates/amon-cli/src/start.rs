//! `amon start` — the agents you started, and starting one of them again
//! (ADR-0026).
//!
//! Without arguments it lists the history the wrappers record, the way the
//! start agent panel shows it; `--json` is what the panel reads. With an agent
//! and a folder it opens a new terminal there running that agent - bare, never
//! with the arguments it last had - and for a remote entry it reaches the
//! machine through the ssh session it was last reached by.
//!
//! An entry is named by what makes it one (agent, folder, host) rather than by
//! its place in the list, so a pick cannot land on another row because an
//! agent started somewhere in between.

use std::path::Path;
use std::process::{Command, Stdio};

use amon_protocol::started::{self, Started};
use serde::Serialize;

/// The window class Omarchy gives its agent terminals, so a relaunched agent
/// gets the same window rules as one from Omarchy's own key.
const APP_ID: &str = "org.omarchy.agent";

/// One row, ready to draw: the panel does no formatting of its own.
#[derive(Debug, Serialize, PartialEq)]
pub struct Row {
    pub agent: String,
    pub dir: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    /// The repository's name, bold in the panel; absent outside one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subpath: Option<String>,
    /// The folder with the home as `~`, for a row outside a repository.
    pub path: String,
    /// "5 min ago", "yesterday", "Sep 28".
    pub when: String,
}

/// The rows the panel shows: the history, most recent first, without local
/// folders that are gone - hidden, not forgotten, so a folder that comes back
/// brings its row back (ADR-0026).
pub fn rows(entries: &[Started], now: i64, home: &str, exists: impl Fn(&str) -> bool) -> Vec<Row> {
    entries
        .iter()
        .filter(|entry| entry.host.is_some() || exists(&entry.dir))
        .take(started::CAP)
        .map(|entry| Row {
            agent: entry.agent.clone(),
            dir: entry.dir.clone(),
            host: entry.host.clone(),
            project: entry.project.clone(),
            subpath: entry.subpath.clone(),
            path: started::display_dir(&entry.dir, entry.host.is_some(), home),
            when: started::when(entry.last_started_secs(), now),
        })
        .collect()
}

pub fn list(json: bool) -> Result<(), Box<dyn std::error::Error>> {
    let home = std::env::var("HOME").unwrap_or_default();
    let rows = rows(&started::load(), now_secs(), &home, |dir| {
        Path::new(dir).is_dir()
    });
    if json {
        println!("{}", serde_json::to_string(&rows)?);
        return Ok(());
    }
    if rows.is_empty() {
        println!("no agents started yet");
        return Ok(());
    }
    for row in rows {
        let place = match (&row.project, &row.subpath) {
            (Some(project), Some(subpath)) => format!("{project}/{subpath}"),
            (Some(project), None) => project.clone(),
            (None, _) => row.path.clone(),
        };
        let place = match &row.host {
            Some(host) => format!("{host}: {place}"),
            None => place,
        };
        println!("{:<14} {:<40} {}", row.agent, place, row.when);
    }
    Ok(())
}

/// Starts `agent` in `dir` again - on `host`, through its recorded ssh
/// arguments, when one is named.
pub fn launch(
    agent: &str,
    dir: &str,
    host: Option<&str>,
) -> Result<(), Box<dyn std::error::Error>> {
    let amon = amon_protocol::paths::own_binary();
    let command = match host {
        None => {
            if !Path::new(dir).is_dir() {
                return Err(format!("{dir} no longer exists").into());
            }
            local_command(&amon.to_string_lossy(), agent, dir)
        }
        Some(host) => {
            let entry = started::load()
                .into_iter()
                .find(|entry| entry.key() == (agent, dir, Some(host)))
                .ok_or_else(|| format!("no record of {agent} in {dir} on {host}"))?;
            remote_command(&amon.to_string_lossy(), agent, dir, &entry.ssh)
        }
    };
    spawn_terminal(&command)
}

/// What the new terminal runs for a local entry: the terminal itself opens in
/// the folder.
fn local_command(amon: &str, agent: &str, dir: &str) -> Vec<String> {
    vec![
        format!("--dir={dir}"),
        format!("--app-id={APP_ID}"),
        "-e".into(),
        amon.into(),
        agent.into(),
    ]
}

/// What the new terminal runs for a remote entry: a wrapped ssh with the same
/// arguments, forced onto a terminal, that changes to the folder there and
/// starts the agent through the remote user's login shell - where their own
/// aliases put it under their own amon.
fn remote_command(amon: &str, agent: &str, dir: &str, ssh: &[String]) -> Vec<String> {
    let mut command = vec![
        format!("--app-id={APP_ID}"),
        "-e".into(),
        amon.into(),
        "ssh".into(),
        "-t".into(),
    ];
    command.extend(ssh.iter().cloned());
    command.push(format!(
        "cd {} && exec \"$SHELL\" -lic {}",
        quote(dir),
        quote(agent)
    ));
    command
}

/// One word for a POSIX shell, whatever it contains.
fn quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', r"'\''"))
}

/// Opens the user's terminal through Omarchy's own path - `uwsm-app` so it is
/// a proper app unit - detached, so it outlives this command. Falls back to
/// `xdg-terminal-exec` directly where uwsm is not installed.
fn spawn_terminal(arguments: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let uwsm = which("uwsm-app");
    let mut command = Command::new("setsid");
    if uwsm {
        command.args(["uwsm-app", "--"]);
    }
    command
        .arg("xdg-terminal-exec")
        .args(arguments)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    command.spawn()?;
    Ok(())
}

fn which(program: &str) -> bool {
    std::env::var_os("PATH")
        .is_some_and(|path| std::env::split_paths(&path).any(|dir| dir.join(program).is_file()))
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(agent: &str, dir: &str, host: Option<&str>, secs: i64) -> Started {
        Started {
            agent: agent.into(),
            dir: dir.into(),
            project: None,
            subpath: None,
            host: host.map(str::to_owned),
            ssh: Vec::new(),
            last_started: started::datetime_from_unix(secs),
        }
    }

    #[test]
    fn rows_hide_local_folders_that_are_gone_but_keep_remote_ones() {
        let entries = vec![
            entry("claude", "/home/me/here", None, 300),
            entry("codex", "/home/me/gone", None, 200),
            entry("claude", "/Users/me/far", Some("mac"), 100),
        ];
        let rows = rows(&entries, 400, "/home/me", |dir| dir != "/home/me/gone");
        let shown: Vec<_> = rows
            .iter()
            .map(|row| (row.agent.as_str(), row.path.as_str()))
            .collect();
        assert_eq!(shown, [("claude", "~/here"), ("claude", "~/far")]);
        assert_eq!(rows[1].host.as_deref(), Some("mac"));
        assert_eq!(rows[0].when, "1 min ago");
    }

    #[test]
    fn a_local_start_opens_the_terminal_in_the_folder_with_the_bare_agent() {
        assert_eq!(
            local_command("/bin/amon", "claude", "/home/me/x"),
            [
                "--dir=/home/me/x",
                "--app-id=org.omarchy.agent",
                "-e",
                "/bin/amon",
                "claude"
            ]
        );
    }

    #[test]
    fn a_remote_start_reaches_the_host_again_and_starts_the_agent_there() {
        let command = remote_command(
            "/bin/amon",
            "claude",
            "/Users/me/it's here",
            &["-p".into(), "2222".into(), "me@mac".into()],
        );
        assert_eq!(
            command,
            [
                "--app-id=org.omarchy.agent",
                "-e",
                "/bin/amon",
                "ssh",
                "-t",
                "-p",
                "2222",
                "me@mac",
                r#"cd '/Users/me/it'\''s here' && exec "$SHELL" -lic 'claude'"#,
            ]
        );
    }
}
