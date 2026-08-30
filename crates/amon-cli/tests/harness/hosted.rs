//! A real herdr or luvus session inside the sandbox.
//!
//! The runtime's client runs on a harness-owned PTY with the sandbox's HOME,
//! so its server, sockets and state all live under the sandbox and the
//! daemon under test finds it through the same `/proc` walk it uses on a
//! desktop. The fake agent is the pane *shell*: typing a command into a
//! fresh pane races the shell's startup in both runtimes, while a pane whose
//! shell is the agent runs it the instant the pane exists, and both runtimes
//! detect it natively from the process tree.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use super::{wait_for_within, PtySession, Sandbox};

/// Long enough for a runtime to start, spawn a pane, and run its detection
/// through two working stints; short enough that a genuine hang fails the
/// test rather than the suite.
pub const RUNTIME_DEADLINE: Duration = Duration::from_secs(40);

const SESSION: &str = "amon-e2e";

/// A fake agent that works twice.
///
/// Twice, because herdr suppresses the completion that immediately follows
/// acquiring an agent — a launching CLI's welcome screen must not ring — so
/// only the second stint's finish counts as "done". luvus counts the first,
/// and is not bothered by a second. It then sleeps long enough to outlive
/// any test, and the sandbox tears it down.
pub const WORKS_TWICE: &str = r#"#!/bin/sh
work() {
    printf '\033]0;\342\240\213 working\007'
    echo working
    sleep 4
    printf '\033]0;\342\234\263 done\007'
    echo finished
}
work
sleep 4
work
sleep 600
"#;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Herdr,
    Luvus,
}

impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Kind::Herdr => "herdr",
            Kind::Luvus => "luvus",
        }
    }
}

/// The version `.mise.toml` pins for this runtime.
fn pinned_version(kind: Kind) -> String {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let pins = std::fs::read_to_string(root.join(".mise.toml")).expect(".mise.toml");
    pins.lines()
        .find(|line| line.contains(&format!("/{}\"", kind.name())))
        .and_then(|line| line.split('"').nth(3))
        .map(str::to_owned)
        .unwrap_or_else(|| panic!("{} is not pinned in .mise.toml", kind.name()))
}

/// The pinned runtime binary: what mise resolves for the workspace, or
/// failing that the first non-shim binary on PATH — and in either case the
/// version `.mise.toml` names, or the test is not testing what it says.
/// (A shim would resolve against the sandboxed HOME and find nothing.)
pub fn binary(kind: Kind) -> PathBuf {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let from_mise = Command::new("mise")
        .args(["which", kind.name()])
        .current_dir(&root)
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| PathBuf::from(String::from_utf8_lossy(&output.stdout).trim()))
        .filter(|found| found.is_file());
    let from_path = || {
        let path = std::env::var_os("PATH").unwrap_or_default();
        std::env::split_paths(&path)
            .map(|dir| dir.join(kind.name()))
            .find(|candidate| {
                candidate.is_file() && !candidate.to_string_lossy().contains("/shims/")
            })
    };
    let Some(found) = from_mise.or_else(from_path) else {
        panic!(
            "{} is not installed; run `mise install` (it is pinned in .mise.toml)",
            kind.name()
        );
    };
    let pinned = pinned_version(kind);
    let version = Command::new(&found)
        .arg("--version")
        .output()
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_owned())
        .unwrap_or_default();
    assert!(
        version.ends_with(&pinned),
        "{} at {} is `{version}`, not the pinned {pinned}; run `mise install`",
        kind.name(),
        found.display()
    );
    found
}

pub struct Hosted {
    pub kind: Kind,
    /// The session's API socket.
    pub socket: PathBuf,
    binary: PathBuf,
    client: PtySession,
    env: Vec<(String, String)>,
}

impl Hosted {
    /// Starts `<runtime> --session amon-e2e` in the sandbox with `shell` as
    /// every pane's shell, and waits until its API socket answers.
    pub fn start(sandbox: &Sandbox, kind: Kind, shell: &Path) -> Self {
        let binary = binary(kind);
        let socket = match kind {
            Kind::Herdr => {
                sandbox.home_path(&format!(".config/herdr/sessions/{SESSION}/herdr.sock"))
            }
            Kind::Luvus => sandbox.home_path(&format!(".luvus/sessions/{SESSION}/luvus.sock")),
        };
        let mut env = vec![
            ("SHELL".to_string(), shell.to_string_lossy().into_owned()),
            ("TERM".to_string(), "xterm-256color".to_string()),
        ];
        if kind == Kind::Luvus {
            env.push((
                "LUVUS_HOME".to_string(),
                sandbox.home_path(".luvus").to_string_lossy().into_owned(),
            ));
        }
        let client = Self::attach(sandbox, &binary, &env);
        let hosted = Self {
            kind,
            socket,
            binary,
            client,
            env,
        };
        wait_for_within("the runtime's socket", RUNTIME_DEADLINE, || {
            hosted.try_request("ping", serde_json::json!({})).ok()
        });
        hosted
    }

    fn attach(sandbox: &Sandbox, binary: &Path, env: &[(String, String)]) -> PtySession {
        let env: Vec<(&str, &str)> = env
            .iter()
            .map(|(name, value)| (name.as_str(), value.as_str()))
            .collect();
        PtySession::start_program(sandbox, binary, &["--session", SESSION], &env)
    }

    /// Opens a pane the user is not looking at, and returns its id.
    ///
    /// herdr counts a whole tab as seen while it is active, so "not looked
    /// at" is a background tab there; luvus tracks seen per pane, so a
    /// split that keeps focus where it was is enough.
    pub fn background_pane(&self) -> String {
        match self.kind {
            Kind::Herdr => {
                let panes = self.request("pane.list", serde_json::json!({}));
                let workspace = panes["panes"][0]["workspace_id"]
                    .as_str()
                    .expect("the first pane names its workspace")
                    .to_owned();
                let created = self.request(
                    "tab.create",
                    serde_json::json!({"workspace_id": workspace, "focus": false}),
                );
                created["root_pane"]["pane_id"]
                    .as_str()
                    .expect("a created tab has a root pane")
                    .to_owned()
            }
            Kind::Luvus => {
                let created = self.request(
                    "pane.split",
                    serde_json::json!({"direction": "right", "focus": false}),
                );
                created["pane"]
                    .as_str()
                    .expect("a split names the new pane")
                    .to_owned()
            }
        }
    }

    /// Reports `status` for `pane` through the runtime's own integration
    /// call, the way a hook would.
    pub fn report(&self, pane: &str, status: &str) {
        match self.kind {
            Kind::Herdr => self.request(
                "pane.report_agent",
                serde_json::json!({"pane_id": pane, "source": "amon-e2e", "agent": "claude", "state": status}),
            ),
            Kind::Luvus => self.request(
                "agent.report",
                serde_json::json!({"pane": pane, "source": "amon-e2e", "agent": "claude", "status": status}),
            ),
        };
    }

    /// `agent.list` as the runtime sees it: (pane, status) pairs.
    pub fn agents(&self) -> Vec<(String, String)> {
        let list = self.request("agent.list", serde_json::json!({}));
        let (pane_key, status_key) = match self.kind {
            Kind::Herdr => ("pane_id", "agent_status"),
            Kind::Luvus => ("pane", "status"),
        };
        list["agents"]
            .as_array()
            .map(|agents| {
                agents
                    .iter()
                    .filter_map(|agent| {
                        Some((
                            agent[pane_key].as_str()?.to_owned(),
                            agent[status_key].as_str()?.to_owned(),
                        ))
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The runtime's status for one pane, if it holds an agent there.
    pub fn status_of(&self, pane: &str) -> Option<String> {
        self.agents()
            .into_iter()
            .find(|(p, _)| p == pane)
            .map(|(_, status)| status)
    }

    /// Kills the client. The server, detached under its own session, keeps
    /// running with every pane intact.
    pub fn detach(&mut self) {
        self.client.kill();
    }

    pub fn reattach(&mut self, sandbox: &Sandbox) {
        self.client = Self::attach(sandbox, &self.binary, &self.env);
    }

    pub fn request(&self, method: &str, params: serde_json::Value) -> serde_json::Value {
        self.try_request(method, params)
            .unwrap_or_else(|error| panic!("{method} against {}: {error}", self.kind.name()))
    }

    fn try_request(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> std::io::Result<serde_json::Value> {
        let stream = UnixStream::connect(&self.socket)?;
        stream.set_read_timeout(Some(Duration::from_secs(5)))?;
        stream.set_write_timeout(Some(Duration::from_secs(5)))?;
        let mut writer = stream.try_clone()?;
        let line = serde_json::json!({"id": "e2e", "method": method, "params": params});
        writeln!(writer, "{line}")?;
        let mut reply = String::new();
        BufReader::new(stream).read_line(&mut reply)?;
        let reply: serde_json::Value = serde_json::from_str(&reply)
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
        if let Some(error) = reply.get("error") {
            return Err(std::io::Error::other(error.to_string()));
        }
        reply
            .get("result")
            .cloned()
            .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidData, "no result"))
    }
}

impl Drop for Hosted {
    fn drop(&mut self) {
        // Ask the server to stop, so its panes — and the fake agents in them,
        // each sleeping for ten minutes — go with it rather than outliving
        // the test. Both runtimes answer `server.stop`; a server that is
        // already gone simply does not answer.
        let _ = self.try_request("server.stop", serde_json::json!({}));
        self.client.kill();
    }
}
