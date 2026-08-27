# Herdr Live Integration Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Agents running inside herdr sessions appear in amon's registry (bar, panel, `amon status`, `amon focus`) without wrapping herdr, and `amon focus` lands on the exact pane.

**Architecture:** A `herdr` module inside `amon-daemon` polls `/proc` for attached herdr TUI clients, and per live session opens the session's JSON socket (`session.snapshot` + `events.subscribe`), projecting herdr agents into the existing `Registry` via synthetic connection ids. Window/workspace comes from the client pid through the same Hyprland ancestry walk the wrapper uses today, extracted into a shared `amon-hypr` crate. `AgentEntry` gains one optional `herdr` field so the CLI can do the second focus hop (`agent.focus`) and consumers can tell herdr agents apart.

**Tech Stack:** Rust (sync std threads + unix sockets, no async — matches daemon style), serde/serde_json, newline-delimited JSON both toward the daemon's subscribers and toward herdr.

**Spec:** `docs/research/herdr-live-integration.md` (decisions: hide sessions with no attached client; first client wins on multi-attach; live tracking at both Hyprland and herdr layers; herdr provenance carried on the entry).

## Global Constraints

- Tests run with `cargo nextest run --workspace --status-level fail` (`just test`), never plain `cargo test` (env-mutating vendored tests need a process per test). Doc tests via `cargo test --workspace --doc`.
- Lint gate: `cargo clippy --workspace --all-targets -- -D warnings` and `cargo fmt --all --check` (`just lint`).
- Protocol changes are additive-only; after touching `amon-protocol` types run `just schema` and commit the regenerated `docs/protocol.schema.json`.
- Commit messages are plain sentences (repo style, e.g. "A new share card, and the card decides the format"), no conventional-commit prefixes, and **never credit Claude or add Co-Authored-By**.
- Comments follow the repo's narrative style: explain the constraint the code can't show, not what the next line does.
- herdr protocol facts this plan relies on (verified against herdr 0.8.2 source, see spec): NDJSON request `{"id","method","params"}` / response `{"id","result":{...}}` or `{"id","error":{...}}`; one request per connection except `events.subscribe`, which acks then streams `{"event":"...","data":{...}}` lines; sockets at `$XDG_CONFIG_HOME/herdr/herdr.sock` (default session) or `.../herdr/sessions/<name>/herdr.sock`; agent statuses `idle|working|blocked|done|unknown`.

---

### Task 1: Extract `amon-hypr` from the wrapper

The daemon needs the ancestry→window resolution and the follow loop; today they live in `crates/amon-wrapper/src/hypr.rs` coupled to `observer::Signal`. Move the whole module into a new crate with a signal-agnostic `follow`, and leave the wrapper a thin adapter.

**Files:**
- Create: `crates/amon-hypr/Cargo.toml`, `crates/amon-hypr/src/lib.rs`
- Modify: `Cargo.toml` (workspace members + `[workspace.dependencies]`), `crates/amon-wrapper/Cargo.toml`, `crates/amon-wrapper/src/hypr.rs`
- Test: unit tests move with the code into `crates/amon-hypr/src/lib.rs`

**Interfaces:**
- Produces (used by Tasks 6 and by the wrapper):
  - `pub struct Window { pub address: String, pub workspace: Option<String> }`
  - `pub fn socket_dir() -> Option<PathBuf>`
  - `pub fn connect_events(directory: &Path) -> std::io::Result<UnixStream>` (connects `.socket2.sock`)
  - `pub fn resolve(directory: &Path, pid: u32) -> Option<Window>`
  - `pub fn resolve_when_mapped(directory: &Path, pid: u32) -> Option<Window>`
  - `pub fn follow(directory: &Path, events: UnixStream, window: Window, on_change: impl FnMut(Option<Window>))` — calls `on_change(Some(w))` on workspace moves and re-resolves once on close, `on_change(None)` when the window is gone; returns when the stream ends.
  - `pub fn window_for_ancestry(clients_json: &str, ancestry: &[u32]) -> Option<Window>`, `pub fn ancestry(pid: u32) -> Vec<u32>`, `pub fn parse_event(line: &str) -> Option<Event>`, `pub fn parse_ppid(stat: &str) -> Option<u32>` (public for tests and the daemon)

- [ ] **Step 1: Create the crate and move the code**

`crates/amon-hypr/Cargo.toml`:

```toml
[package]
name = "amon-hypr"
version.workspace = true
edition.workspace = true
license.workspace = true

[dependencies]
serde_json = { workspace = true }
```

(Match the `[package]` inheritance keys the other crates use — open `crates/amon-protocol/Cargo.toml` and copy its shape.)

`crates/amon-hypr/src/lib.rs`: move the entire contents of `crates/amon-wrapper/src/hypr.rs` except `spawn` (stays in the wrapper) and rework `follow` to take a callback instead of a `Sender<Signal>`:

```rust
/// Reads the event stream, keeping the window's workspace current.
///
/// `on_change(Some(window))` on every workspace move and on finding a
/// replacement window after a close; `on_change(None)` when the window is
/// gone for good. Returns when the event stream ends.
pub fn follow(
    directory: &Path,
    events: UnixStream,
    mut window: Window,
    mut on_change: impl FnMut(Option<Window>),
) {
    let mut reader = BufReader::new(events);
    let mut line = String::new();
    loop {
        line.clear();
        match reader.by_ref().take(MAX_EVENT_LINE).read_line(&mut line) {
            Ok(0) | Err(_) => return,
            Ok(_) => {}
        }
        match parse_event(line.trim_end()) {
            Some(Event::Moved { address, workspace }) if address == window.address => {
                if window.workspace.as_deref() == Some(workspace.as_str()) {
                    continue;
                }
                window.workspace = Some(workspace);
                on_change(Some(window.clone()));
            }
            Some(Event::Closed { address }) if address == window.address => {
                match resolve(directory, std::process::id()) {
                    Some(found) => {
                        window = found;
                        on_change(Some(window.clone()));
                    }
                    None => {
                        on_change(None);
                        return;
                    }
                }
            }
            _ => {}
        }
    }
}
```

Wait — `follow` re-resolves with `std::process::id()`, which is wrapper-specific. Make the pid a parameter so the daemon can pass the herdr client's pid:

```rust
pub fn follow(
    directory: &Path,
    events: UnixStream,
    pid: u32,
    mut window: Window,
    mut on_change: impl FnMut(Option<Window>),
)
```

and use `resolve(directory, pid)` in the `Closed` arm. Add:

```rust
/// Connects the compositor's event stream. Subscribe before resolving, so a
/// move during the lookup queues rather than being lost.
pub fn connect_events(directory: &Path) -> std::io::Result<UnixStream> {
    UnixStream::connect(directory.join(".socket2.sock"))
}
```

Make `socket_dir`, `resolve`, `resolve_when_mapped`, `ancestry` `pub`. Move the whole `#[cfg(test)] mod tests` along verbatim.

- [ ] **Step 2: Rewire the wrapper**

`crates/amon-wrapper/src/hypr.rs` shrinks to the adapter (and a `pub use` so other wrapper modules keep compiling if they name these types):

```rust
//! Which window the agent is running in — the wrapper's thin adapter over
//! `amon_hypr`, which owns the walk and the event following.

use std::sync::mpsc::Sender;

use amon_hypr::Window;

use crate::observer::Signal;

pub fn spawn(signals: Sender<Signal>) {
    std::thread::spawn(move || {
        let Some(directory) = amon_hypr::socket_dir() else {
            return;
        };
        let Ok(events) = amon_hypr::connect_events(&directory) else {
            return;
        };
        let pid = std::process::id();
        let Some(window) = amon_hypr::resolve_when_mapped(&directory, pid) else {
            return;
        };
        let _ = signals.send(Signal::Window {
            window: Some(window.address.clone()),
            workspace: window.workspace.clone(),
        });
        amon_hypr::follow(&directory, events, pid, window, |window| {
            let _ = signals.send(match window {
                Some(window) => Signal::Window {
                    window: Some(window.address),
                    workspace: window.workspace,
                },
                None => Signal::Window {
                    window: None,
                    workspace: None,
                },
            });
        });
    });
}
```

Root `Cargo.toml`: add `"crates/amon-hypr"` to `[workspace] members` (if members are listed explicitly — check; a `crates/*` glob needs nothing) and `amon-hypr = { path = "crates/amon-hypr" }` to `[workspace.dependencies]`. `crates/amon-wrapper/Cargo.toml`: add `amon-hypr = { workspace = true }`.

Search the wrapper for other users of the moved items (`grep -rn "hypr::" crates/amon-wrapper/src`) and update paths (`hypr::Window` → `amon_hypr::Window` where needed).

- [ ] **Step 3: Run the full test suite**

Run: `cargo nextest run --workspace --status-level fail`
Expected: PASS — the moved unit tests (`the_nearest_ancestor_that_is_a_window_wins` etc.) now run from `amon-hypr`; nothing else changed behavior.

- [ ] **Step 4: Lint**

Run: `cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --all --check`
Expected: clean.

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml Cargo.lock crates/amon-hypr crates/amon-wrapper
git commit -m "The window walk moves to its own crate, so the daemon can use it"
```

---

### Task 2: `herdr` provenance on `AgentEntry`

**Files:**
- Modify: `crates/amon-protocol/src/agent.rs`, `crates/amon-protocol/src/lib.rs` (export)
- Test: `crates/amon-protocol/tests/protocol.rs`, regenerate `docs/protocol.schema.json`

**Interfaces:**
- Produces: `pub struct HerdrInfo { pub socket: String, pub session: Option<String>, pub pane: String }`, `AgentEntry.herdr: Option<HerdrInfo>`. Consumed by Tasks 5 (projection fills it) and 8 (focus hop reads it).

- [ ] **Step 1: Write the failing test**

In `crates/amon-protocol/tests/protocol.rs` (append, following the file's existing test style):

```rust
#[test]
fn a_herdr_entry_round_trips_and_a_plain_one_stays_bare() {
    let mut entry = AgentEntry {
        id: "herdr:default:term_1".into(),
        agent: "claude".into(),
        state: AgentState::Idle,
        state_since: 1,
        cwd: "/".into(),
        pid: 0,
        args: vec!["claude".into()],
        hostname: "host".into(),
        started_at: 1,
        title: None,
        agent_session_id: None,
        agent_session_path: None,
        window: None,
        workspace: None,
        branch: None,
        focused: None,
        seen: None,
        herdr: Some(HerdrInfo {
            socket: "/home/me/.config/herdr/herdr.sock".into(),
            session: None,
            pane: "w1:p2".into(),
        }),
    };
    let json = serde_json::to_value(&entry).unwrap();
    assert_eq!(json["herdr"]["pane"], "w1:p2");
    let back: AgentEntry = serde_json::from_value(json).unwrap();
    assert_eq!(back, entry);

    // Absent on the wire when not set, so old consumers see nothing new.
    entry.herdr = None;
    let json = serde_json::to_value(&entry).unwrap();
    assert!(json.get("herdr").is_none());
}
```

(If the existing tests build `AgentEntry` through a helper, use that helper instead of the literal — read the top of the file first.)

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo nextest run -p amon-protocol --status-level fail`
Expected: FAIL — `HerdrInfo` not found / missing field `herdr`.

- [ ] **Step 3: Implement**

In `crates/amon-protocol/src/agent.rs`:

```rust
/// Where a herdr-hosted agent lives inside herdr. Present exactly on entries
/// the daemon's herdr module projects; wrapped agents never carry it. The
/// socket travels with the entry so a consumer that wants to act on the agent
/// (the focus hop) does not re-derive herdr's path rules.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct HerdrInfo {
    /// Absolute path of the session's JSON API socket.
    pub socket: String,
    /// Session name; absent for herdr's default session.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
    /// The pane currently hosting the agent (`w1:p1`) — what `agent.focus`
    /// takes. Pane ids change when panes move across herdr workspaces, so the
    /// daemon refreshes this and consumers must not cache it.
    pub pane: String,
}
```

On `AgentEntry`, after `seen`:

```rust
    /// Set when this agent runs inside a herdr session rather than under an
    /// amon wrapper.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub herdr: Option<HerdrInfo>,
```

Fix every struct-literal construction of `AgentEntry` the workspace now fails on (`cargo check --workspace` lists them; add `herdr: None`). Export `HerdrInfo` from `crates/amon-protocol/src/lib.rs` next to `AgentEntry`.

- [ ] **Step 4: Run tests, regenerate the schema**

Run: `cargo nextest run --workspace --status-level fail`
Expected: PASS (the schema test may fail until regenerated — that is what the next command fixes).
Run: `UPDATE_SCHEMA=1 cargo test -p amon-protocol --test schema` (i.e. `just schema`), then `cargo nextest run -p amon-protocol --status-level fail` again.
Expected: PASS, `docs/protocol.schema.json` modified.

- [ ] **Step 5: Commit**

```bash
git add crates/amon-protocol crates docs/protocol.schema.json Cargo.lock
git commit -m "An entry can say it lives in herdr, and where"
```

---

### Task 3: Classify herdr processes from /proc

**Files:**
- Create: `crates/amon-daemon/src/herdr/mod.rs` (module skeleton: `mod proc; mod wire; mod project;` — add submodule declarations as tasks land), `crates/amon-daemon/src/herdr/proc.rs`
- Modify: `crates/amon-daemon/src/lib.rs` (add `mod herdr;`)
- Test: inline `#[cfg(test)]` in `proc.rs`

**Interfaces:**
- Produces (consumed by Task 6):
  - `pub struct HerdrSession { pub name: Option<String>, pub socket: PathBuf, pub client_pid: u32 }`
  - `pub fn sessions() -> Vec<HerdrSession>` — scans `/proc`, one entry per session that has at least one attached client, first (lowest-pid) client wins.
  - Pure, tested: `pub fn classify(argv: &[String], is_session_leader: bool, has_tty: bool, environ: &str) -> Role`, `pub enum Role { Client { name: Option<String> }, Server { name: Option<String> }, Other }`, `pub fn session_socket(name: Option<&str>) -> PathBuf`, `pub fn stat_fields(stat: &str) -> Option<(u32, u32)>` (returns `(session_id_field, tty_nr)`).

- [ ] **Step 1: Write the failing tests**

`crates/amon-daemon/src/herdr/proc.rs`, tests first:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn argv(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn a_bare_herdr_with_a_tty_is_a_client_of_the_default_session() {
        assert_eq!(
            classify(&argv(&["/usr/bin/herdr"]), false, true, ""),
            Role::Client { name: None }
        );
    }

    #[test]
    fn the_session_flag_names_the_session_in_both_spellings() {
        assert_eq!(
            classify(&argv(&["herdr", "--session", "work"]), false, true, ""),
            Role::Client { name: Some("work".into()) }
        );
        assert_eq!(
            classify(&argv(&["herdr", "--session=work"]), false, true, ""),
            Role::Client { name: Some("work".into()) }
        );
        assert_eq!(
            classify(&argv(&["herdr", "session", "attach", "work"]), false, true, ""),
            Role::Client { name: Some("work".into()) }
        );
    }

    #[test]
    fn attach_forms_and_the_explicit_client_are_clients_too() {
        for form in [
            vec!["herdr", "client"],
            vec!["herdr", "agent", "attach", "reviewer"],
            vec!["herdr", "terminal", "attach", "term_1"],
            vec!["herdr", "--no-session"],
        ] {
            assert!(matches!(
                classify(&argv(&form), false, true, ""),
                Role::Client { .. }
            ));
        }
    }

    #[test]
    fn the_server_is_a_server_even_though_it_has_no_tty() {
        assert_eq!(
            classify(
                &argv(&["/home/me/.local/bin/herdr", "server"]),
                true,
                false,
                "HERDR_SESSION=work\0HOME=/home/me\0",
            ),
            Role::Server { name: Some("work".into()) }
        );
    }

    #[test]
    fn what_is_neither_is_other() {
        // No tty: a client that lost its terminal is not attached.
        assert_eq!(classify(&argv(&["herdr"]), false, false, ""), Role::Other);
        // The remote wrapper's tty owner is its `herdr client` child.
        assert_eq!(
            classify(&argv(&["herdr", "--remote", "box"]), false, true, ""),
            Role::Other
        );
        // Every other subcommand is a one-shot CLI call.
        assert_eq!(
            classify(&argv(&["herdr", "agent", "list"]), false, true, ""),
            Role::Other
        );
        assert_eq!(
            classify(&argv(&["herdr", "status"]), false, true, ""),
            Role::Other
        );
    }

    #[test]
    fn environ_names_the_session_when_argv_does_not() {
        assert_eq!(
            classify(&argv(&["herdr"]), false, true, "HERDR_SESSION=work\0"),
            Role::Client { name: Some("work".into()) }
        );
    }

    #[test]
    fn the_stat_line_yields_sid_and_tty_past_a_hostile_comm() {
        // comm may contain spaces and parens; fields count from after the
        // last ')'. Field layout: pid (comm) state ppid pgrp session tty_nr…
        let stat = "17 (h (x) dr) S 1 17 17 34816 …";
        assert_eq!(stat_fields(stat), Some((17, 34816)));
    }

    #[test]
    fn the_default_socket_and_a_named_one() {
        let home = std::env::var("HOME").unwrap();
        assert_eq!(
            session_socket(None),
            std::path::PathBuf::from(&home).join(".config/herdr/herdr.sock")
        );
        assert_eq!(
            session_socket(Some("work")),
            std::path::PathBuf::from(&home).join(".config/herdr/sessions/work/herdr.sock")
        );
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo nextest run -p amon-daemon --status-level fail`
Expected: FAIL to compile — module does not exist.

- [ ] **Step 3: Implement**

`crates/amon-daemon/src/herdr/mod.rs` starts as:

```rust
//! Agents living inside herdr sessions, adopted without wrapping herdr.
//!
//! See docs/research/herdr-live-integration.md for the shape of the whole
//! thing and the decisions this implements.

pub mod proc;
```

`crates/amon-daemon/src/herdr/proc.rs`:

```rust
//! Finding herdr on this machine by reading /proc.
//!
//! herdr never rewrites its process title and `--session` survives verbatim
//! in kernel-visible argv, which is what makes classification by argv safe.
//! The server is a session leader with no controlling tty; an attached client
//! is the `herdr` the user typed, tty and all. A session is shown exactly
//! while it has an attached client (spec decision), and of several clients
//! the lowest pid wins — "first, or something" was the whole requirement.

use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Role {
    Client { name: Option<String> },
    Server { name: Option<String> },
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HerdrSession {
    pub name: Option<String>,
    pub socket: PathBuf,
    pub client_pid: u32,
}

pub fn classify(argv: &[String], is_session_leader: bool, has_tty: bool, environ: &str) -> Role {
    let args: Vec<&str> = argv.iter().skip(1).map(String::as_str).collect();
    if args.first() == Some(&"server") {
        // Reached whether or not it has daemonized yet; the tty test would
        // race its own setsid.
        let _ = is_session_leader;
        return Role::Server { name: environ_session(environ) };
    }
    if !has_tty {
        return Role::Other;
    }
    let name = argv_session(&args).or_else(|| environ_session(environ));
    let is_client = match args.split_first() {
        None => true,
        Some((&"--session", _)) => true,
        Some((first, _)) if first.starts_with("--session=") => true,
        Some((&"--no-session", _)) => true,
        Some((&"client", _)) => true,
        Some((&"session", rest)) => rest.first() == Some(&"attach"),
        Some((&"agent", rest)) => rest.first() == Some(&"attach"),
        Some((&"terminal", rest)) => rest.first() == Some(&"attach"),
        Some(_) => false,
    };
    if is_client {
        Role::Client { name }
    } else {
        Role::Other
    }
}

fn argv_session(args: &[&str]) -> Option<String> {
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        if *arg == "--session" {
            return iter.next().map(|s| s.to_string());
        }
        if let Some(name) = arg.strip_prefix("--session=") {
            return Some(name.to_string());
        }
        if *arg == "session" {
            let mut rest = iter.clone();
            if rest.next() == Some(&"attach") {
                return rest.next().map(|s| s.to_string());
            }
        }
    }
    None
}

fn environ_session(environ: &str) -> Option<String> {
    environ
        .split('\0')
        .find_map(|pair| pair.strip_prefix("HERDR_SESSION="))
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
}

/// The session's JSON API socket, by herdr's own layout. `HERDR_SOCKET_PATH`
/// overrides are honored upstream by reading the client's environ; this is
/// the default rule.
pub fn session_socket(name: Option<&str>) -> PathBuf {
    let config = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|dir| !dir.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
        .unwrap_or_default()
        .join("herdr");
    match name {
        None => config.join("herdr.sock"),
        Some(name) => config.join("sessions").join(name).join("herdr.sock"),
    }
}

/// `(session, tty_nr)` from a `/proc/<pid>/stat` line — fields 6 and 7,
/// counted after the last `)` because comm is arbitrary bytes.
pub fn stat_fields(stat: &str) -> Option<(u32, u32)> {
    let after_comm = &stat[stat.rfind(')')? + 1..];
    let mut fields = after_comm.split_whitespace();
    let session = fields.nth(3)?.parse().ok()?;
    let tty_nr = fields.next()?.parse().ok()?;
    Some((session, tty_nr))
}

/// Every herdr session that has an attached client right now.
pub fn sessions() -> Vec<HerdrSession> {
    let mut by_name: std::collections::HashMap<Option<String>, u32> =
        std::collections::HashMap::new();
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return Vec::new();
    };
    for entry in entries.flatten() {
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|name| name.parse::<u32>().ok())
        else {
            continue;
        };
        let path = entry.path();
        // comm first: cheap, and filters out everything that is not herdr
        // before argv and environ are read at all.
        let Ok(comm) = std::fs::read_to_string(path.join("comm")) else {
            continue;
        };
        if comm.trim_end() != "herdr" {
            continue;
        }
        let Ok(cmdline) = std::fs::read(path.join("cmdline")) else {
            continue;
        };
        let argv: Vec<String> = cmdline
            .split(|byte| *byte == 0)
            .filter(|part| !part.is_empty())
            .map(|part| String::from_utf8_lossy(part).into_owned())
            .collect();
        let Ok(stat) = std::fs::read_to_string(path.join("stat")) else {
            continue;
        };
        let Some((session_id, tty_nr)) = stat_fields(&stat) else {
            continue;
        };
        let environ = std::fs::read(path.join("environ")).unwrap_or_default();
        let environ = String::from_utf8_lossy(&environ).into_owned();
        if let Role::Client { name } = classify(&argv, session_id == pid, tty_nr != 0, &environ) {
            let slot = by_name.entry(name).or_insert(pid);
            *slot = (*slot).min(pid);
        }
    }
    by_name
        .into_iter()
        .map(|(name, client_pid)| HerdrSession {
            socket: session_socket(name.as_deref()),
            name,
            client_pid,
        })
        .collect()
}
```

Add `mod herdr;` to `crates/amon-daemon/src/lib.rs` next to the other `mod` lines.

- [ ] **Step 4: Run tests**

Run: `cargo nextest run -p amon-daemon --status-level fail`
Expected: PASS. (`sessions()` reads the live `/proc` and is exercised for real in Task 6's test only through classification of a fake — it stays untested here by design; everything decision-bearing is pure and tested.)

- [ ] **Step 5: Commit**

```bash
git add crates/amon-daemon
git commit -m "The daemon can tell a herdr client from its server by reading proc"
```

---

### Task 4: A blocking herdr socket client

**Files:**
- Create: `crates/amon-daemon/src/herdr/wire.rs`
- Modify: `crates/amon-daemon/src/herdr/mod.rs` (`pub mod wire;`)
- Test: inline `#[cfg(test)]` with a fake herdr server on a tempdir unix socket

**Interfaces:**
- Produces (consumed by Tasks 5, 6, 8):
  - `pub fn request(socket: &Path, method: &str, params: serde_json::Value) -> std::io::Result<serde_json::Value>` — one-shot: connect, send one line, read one line, return the `result` value (io::Error on transport failure or `error` response).
  - `pub struct EventStream(...)` with `pub fn subscribe(socket: &Path, subscriptions: Vec<serde_json::Value>) -> std::io::Result<EventStream>`; `EventStream` implements `Iterator<Item = HerdrEvent>` and `pub fn shutdown_handle(&self) -> ShutdownHandle` (a cloned `UnixStream` whose `shutdown()` unblocks the reader from another thread).
  - `pub struct HerdrEvent { pub event: String, pub data: serde_json::Value }`
  - `pub struct HerdrAgent { pub terminal_id: String, pub agent: String, pub agent_status: String, pub pane_id: String, pub cwd: String, pub title: Option<String> }`
  - `pub fn agents_from(result: &serde_json::Value) -> Vec<HerdrAgent>` — pulls the `agents` array out of a `session.snapshot` result, skipping records missing required fields.

- [ ] **Step 1: Write the failing tests**

In `crates/amon-daemon/src/herdr/wire.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixListener;

    /// One canned herdr: answers the first line per `replies`, then streams
    /// `pushes`, then holds the connection open until dropped.
    fn fake_herdr(
        replies: impl Fn(&str) -> String + Send + 'static,
        pushes: Vec<String>,
    ) -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("herdr.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap_or(0) == 0 {
                    continue;
                }
                let mut stream = stream;
                let _ = writeln!(stream, "{}", replies(line.trim_end()));
                for push in &pushes {
                    let _ = writeln!(stream, "{push}");
                }
                // Held open: subscriptions outlive their pushes.
                std::thread::sleep(std::time::Duration::from_secs(5));
            }
        });
        (dir, socket)
    }

    #[test]
    fn a_request_returns_the_result_value() {
        let (_dir, socket) = fake_herdr(
            |_| r#"{"id":"r","result":{"type":"pong","protocol":21}}"#.into(),
            vec![],
        );
        let result = request(&socket, "ping", serde_json::json!({})).unwrap();
        assert_eq!(result["protocol"], 21);
    }

    #[test]
    fn an_error_response_is_an_error() {
        let (_dir, socket) = fake_herdr(
            |_| r#"{"id":"r","error":{"code":"not_found","message":"no"}}"#.into(),
            vec![],
        );
        assert!(request(&socket, "pane.get", serde_json::json!({})).is_err());
    }

    #[test]
    fn a_subscription_acks_then_streams_events() {
        let (_dir, socket) = fake_herdr(
            |_| r#"{"id":"sub","result":{"type":"subscription_started"}}"#.into(),
            vec![
                r#"{"event":"pane.agent_status_changed","data":{"pane_id":"w1:p1","agent_status":"blocked"}}"#.into(),
            ],
        );
        let mut events = subscribe(&socket, vec![serde_json::json!({"type":"pane.closed"})]).unwrap();
        let event = events.next().unwrap();
        assert_eq!(event.event, "pane.agent_status_changed");
        assert_eq!(event.data["agent_status"], "blocked");
    }

    #[test]
    fn shutdown_unblocks_a_waiting_reader() {
        let (_dir, socket) = fake_herdr(
            |_| r#"{"id":"sub","result":{"type":"subscription_started"}}"#.into(),
            vec![],
        );
        let mut events = subscribe(&socket, vec![]).unwrap();
        let handle = events.shutdown_handle();
        let reader = std::thread::spawn(move || events.next());
        handle.shutdown();
        assert!(reader.join().unwrap().is_none());
    }

    #[test]
    fn snapshot_agents_are_extracted_and_partial_records_skipped() {
        let snapshot: serde_json::Value = serde_json::json!({
            "type": "session_snapshot",
            "agents": [
                {"terminal_id":"t1","agent":"claude","agent_status":"working",
                 "pane_id":"w1:p1","cwd":"/repo","title":"fixing"},
                {"agent":"mystery"}
            ]
        });
        let agents = agents_from(&snapshot);
        assert_eq!(agents.len(), 1);
        assert_eq!(agents[0].terminal_id, "t1");
        assert_eq!(agents[0].title.as_deref(), Some("fixing"));
    }
}
```

Add `tempfile` to `crates/amon-daemon/Cargo.toml` `[dev-dependencies]` (it is already a workspace dependency — check with `grep tempfile Cargo.toml`; if not, add it to `[workspace.dependencies]` too).

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo nextest run -p amon-daemon --status-level fail`
Expected: FAIL to compile.

- [ ] **Step 3: Implement**

```rust
//! Speaking herdr's socket: newline-delimited JSON, one request per
//! connection, except a subscription, which acks and then streams.
//!
//! Deliberately raw the whole way down — the daemon never shells out to the
//! `herdr` CLI, whose protocol guard hard-fails on any version mismatch,
//! while the socket serves any client and grows additively.

use std::io::{BufRead, BufReader, Write};
use std::net::Shutdown;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::Duration;

const IO_TIMEOUT: Duration = Duration::from_secs(5);
/// Bounds what a peer can make us hold; herdr's own request cap is 1 MiB.
const MAX_LINE: u64 = 8 * 1024 * 1024;

pub struct HerdrEvent {
    pub event: String,
    pub data: serde_json::Value,
}

#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
pub struct HerdrAgent {
    pub terminal_id: String,
    pub agent: String,
    pub agent_status: String,
    pub pane_id: String,
    #[serde(default)]
    pub cwd: String,
    #[serde(default)]
    pub title: Option<String>,
}

pub fn request(
    socket: &Path,
    method: &str,
    params: serde_json::Value,
) -> std::io::Result<serde_json::Value> {
    let stream = UnixStream::connect(socket)?;
    stream.set_read_timeout(Some(IO_TIMEOUT))?;
    stream.set_write_timeout(Some(IO_TIMEOUT))?;
    let mut writer = stream.try_clone()?;
    let line = serde_json::json!({"id": "amon", "method": method, "params": params});
    writeln!(writer, "{line}")?;
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader.by_ref().take(MAX_LINE).read_line(&mut line)?;
    parse_result(&line)
}

fn parse_result(line: &str) -> std::io::Result<serde_json::Value> {
    let response: serde_json::Value = serde_json::from_str(line)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
    if let Some(error) = response.get("error") {
        return Err(std::io::Error::other(error.to_string()));
    }
    response
        .get("result")
        .cloned()
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidData, "no result"))
}

pub struct EventStream {
    reader: BufReader<UnixStream>,
    stream: UnixStream,
}

pub struct ShutdownHandle(UnixStream);

impl ShutdownHandle {
    pub fn shutdown(&self) {
        let _ = self.0.shutdown(Shutdown::Both);
    }
}

pub fn subscribe(
    socket: &Path,
    subscriptions: Vec<serde_json::Value>,
) -> std::io::Result<EventStream> {
    let stream = UnixStream::connect(socket)?;
    stream.set_write_timeout(Some(IO_TIMEOUT))?;
    let mut writer = stream.try_clone()?;
    let line = serde_json::json!({
        "id": "amon-sub",
        "method": "events.subscribe",
        "params": {"subscriptions": subscriptions},
    });
    writeln!(writer, "{line}")?;
    let mut reader = BufReader::new(stream.try_clone()?);
    // The ack has a deadline; the events after it do not — the stream then
    // blocks for as long as nothing happens, and teardown is the handle's job.
    stream.set_read_timeout(Some(IO_TIMEOUT))?;
    let mut ack = String::new();
    reader.by_ref().take(MAX_LINE).read_line(&mut ack)?;
    parse_result(&ack)?;
    stream.set_read_timeout(None)?;
    Ok(EventStream { reader, stream })
}

impl EventStream {
    pub fn shutdown_handle(&self) -> ShutdownHandle {
        ShutdownHandle(
            self.stream
                .try_clone()
                .expect("cloning a connected unix stream"),
        )
    }
}

impl Iterator for EventStream {
    type Item = HerdrEvent;

    fn next(&mut self) -> Option<HerdrEvent> {
        loop {
            let mut line = String::new();
            match self.reader.by_ref().take(MAX_LINE).read_line(&mut line) {
                Ok(0) | Err(_) => return None,
                Ok(_) => {}
            }
            let Ok(value) = serde_json::from_str::<serde_json::Value>(&line) else {
                continue;
            };
            let Some(event) = value.get("event").and_then(|event| event.as_str()) else {
                continue;
            };
            return Some(HerdrEvent {
                event: event.to_owned(),
                data: value.get("data").cloned().unwrap_or(serde_json::Value::Null),
            });
        }
    }
}

/// The agents out of a `session.snapshot` result. Records missing a required
/// field are herdr's future, not our problem — skipped, never guessed at.
pub fn agents_from(result: &serde_json::Value) -> Vec<HerdrAgent> {
    result
        .get("agents")
        .and_then(|agents| agents.as_array())
        .map(|agents| {
            agents
                .iter()
                .filter_map(|agent| serde_json::from_value(agent.clone()).ok())
                .collect()
        })
        .unwrap_or_default()
}
```

Add `pub mod wire;` to `herdr/mod.rs`. Add `serde = { workspace = true }` to `amon-daemon` dependencies if not present (check the Cargo.toml).

- [ ] **Step 4: Run tests**

Run: `cargo nextest run -p amon-daemon --status-level fail`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/amon-daemon Cargo.lock Cargo.toml
git commit -m "The daemon speaks herdr's socket directly"
```

---

### Task 5: Projecting a herdr agent into an entry

**Files:**
- Create: `crates/amon-daemon/src/herdr/project.rs`
- Modify: `crates/amon-daemon/src/herdr/mod.rs` (`pub mod project;`)
- Test: inline `#[cfg(test)]`

**Interfaces:**
- Consumes: `wire::HerdrAgent` (Task 4), `proc::HerdrSession` (Task 3), `amon_protocol::{AgentEntry, AgentState, HerdrInfo}` (Task 2), `amon_hypr::Window` (Task 1).
- Produces (consumed by Task 6):
  - `pub fn state_for(status: &str) -> (AgentState, Option<bool>)`
  - `pub fn entry_for(agent: &wire::HerdrAgent, session: &proc::HerdrSession, window: Option<&amon_hypr::Window>, now_ms: u64) -> AgentEntry` — id is `herdr:<session-or-default>:<terminal_id>`.

- [ ] **Step 1: Write the failing tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use amon_protocol::AgentState;

    fn agent(status: &str) -> crate::herdr::wire::HerdrAgent {
        crate::herdr::wire::HerdrAgent {
            terminal_id: "t1".into(),
            agent: "claude".into(),
            agent_status: status.into(),
            pane_id: "w1:p2".into(),
            cwd: "/repo".into(),
            title: Some("fixing auth".into()),
        }
    }

    fn session() -> crate::herdr::proc::HerdrSession {
        crate::herdr::proc::HerdrSession {
            name: Some("work".into()),
            socket: "/tmp/herdr.sock".into(),
            client_pid: 42,
        }
    }

    #[test]
    fn herdr_statuses_map_onto_amon_states_and_seen() {
        // done is idle-and-unnoticed — exactly amon's finished-but-unseen.
        assert_eq!(state_for("done"), (AgentState::Idle, Some(false)));
        assert_eq!(state_for("idle"), (AgentState::Idle, Some(true)));
        assert_eq!(state_for("working"), (AgentState::Working, None));
        assert_eq!(state_for("blocked"), (AgentState::Blocked, Some(false)));
        assert_eq!(state_for("unknown"), (AgentState::Unknown, None));
        // A status this build has never heard of must not look urgent.
        assert_eq!(state_for("pondering"), (AgentState::Unknown, None));
    }

    #[test]
    fn an_entry_carries_identity_window_and_the_herdr_pointer() {
        let window = amon_hypr::Window {
            address: "abc123".into(),
            workspace: Some("3".into()),
        };
        let entry = entry_for(&agent("done"), &session(), Some(&window), 1000);
        assert_eq!(entry.id, "herdr:work:t1");
        assert_eq!(entry.agent, "claude");
        assert_eq!(entry.state, AgentState::Idle);
        assert_eq!(entry.seen, Some(false));
        assert_eq!(entry.state_since, 1000);
        assert_eq!(entry.cwd, "/repo");
        assert_eq!(entry.title.as_deref(), Some("fixing auth"));
        assert_eq!(entry.window.as_deref(), Some("abc123"));
        assert_eq!(entry.workspace.as_deref(), Some("3"));
        let herdr = entry.herdr.expect("projected entries carry provenance");
        assert_eq!(herdr.socket, "/tmp/herdr.sock");
        assert_eq!(herdr.session.as_deref(), Some("work"));
        assert_eq!(herdr.pane, "w1:p2");
    }

    #[test]
    fn the_default_session_spells_its_id_with_default() {
        let mut session = session();
        session.name = None;
        let entry = entry_for(&agent("idle"), &session, None, 1);
        assert_eq!(entry.id, "herdr:default:t1");
        assert_eq!(entry.window, None);
        assert_eq!(entry.herdr.unwrap().session, None);
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo nextest run -p amon-daemon --status-level fail`
Expected: FAIL to compile.

- [ ] **Step 3: Implement**

```rust
//! One herdr agent, as an amon entry.
//!
//! The state mapping is the load-bearing part: herdr's `done` is amon's
//! Idle-with-seen-false (finished but unnoticed), and both sides mean the
//! same thing by it because amon vendors herdr's detector (ADR-0001). An
//! unrecognized status maps to Unknown, which ranks last — a new herdr
//! status must not cry wolf on every bar.

use amon_protocol::{AgentEntry, AgentState, HerdrInfo};

use super::{proc::HerdrSession, wire::HerdrAgent};

pub fn state_for(status: &str) -> (AgentState, Option<bool>) {
    match status {
        "idle" => (AgentState::Idle, Some(true)),
        "done" => (AgentState::Idle, Some(false)),
        "working" => (AgentState::Working, None),
        "blocked" => (AgentState::Blocked, Some(false)),
        _ => (AgentState::Unknown, None),
    }
}

pub fn entry_for(
    agent: &HerdrAgent,
    session: &HerdrSession,
    window: Option<&amon_hypr::Window>,
    now_ms: u64,
) -> AgentEntry {
    let (state, seen) = state_for(&agent.agent_status);
    AgentEntry {
        id: format!(
            "herdr:{}:{}",
            session.name.as_deref().unwrap_or("default"),
            agent.terminal_id
        ),
        agent: agent.agent.clone(),
        state,
        state_since: now_ms,
        cwd: agent.cwd.clone(),
        // Not a wrapped process; there is no agent pid amon vouches for.
        pid: 0,
        args: vec![agent.agent.clone()],
        hostname: hostname(),
        started_at: now_ms,
        title: agent.title.clone(),
        agent_session_id: None,
        agent_session_path: None,
        window: window.map(|window| window.address.clone()),
        workspace: window.and_then(|window| window.workspace.clone()),
        branch: None,
        focused: None,
        seen,
        herdr: Some(HerdrInfo {
            socket: session.socket.to_string_lossy().into_owned(),
            session: session.name.clone(),
            pane: agent.pane_id.clone(),
        }),
    }
}

fn hostname() -> String {
    let mut buffer = [0u8; 256];
    if unsafe { libc::gethostname(buffer.as_mut_ptr().cast(), buffer.len()) } != 0 {
        return String::new();
    }
    let end = buffer.iter().position(|byte| *byte == 0).unwrap_or(0);
    String::from_utf8_lossy(&buffer[..end]).into_owned()
}
```

(Reuse the wrapper's `hostname` shape — `crates/amon-wrapper/src/lib.rs:468` — and add `libc = { workspace = true }` and `amon-hypr = { workspace = true }` to `crates/amon-daemon/Cargo.toml`.)

- [ ] **Step 4: Run tests**

Run: `cargo nextest run -p amon-daemon --status-level fail`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/amon-daemon Cargo.lock
git commit -m "A herdr agent projects onto an amon entry, done meaning unseen"
```

---

### Task 6: The session supervisor

The moving part: one watcher thread discovering sessions, one thread per live session owning its herdr connection and its registry entries. Structural herdr events (agents appearing, panes closing or moving) trigger reconnect-and-resnapshot — rare, cheap, and it keeps the event handling to one kind. Status changes flow as patches so the registry's sound machinery sees the crossings.

**Files:**
- Create: `crates/amon-daemon/src/herdr/supervisor.rs`
- Modify: `crates/amon-daemon/src/herdr/mod.rs` (`mod supervisor;` + `pub use supervisor::spawn;`)
- Test: inline `#[cfg(test)]` integration test with the Task 4 fake herdr and a real `Registry`

**Interfaces:**
- Consumes: `Registry::{register, update, disconnect, agents}` (existing), `proc::{sessions, HerdrSession}`, `wire::*`, `project::*`, `amon_hypr::*`.
- Produces (consumed by Task 7):
  - `pub fn spawn(registry: Registry, connections: Arc<AtomicU64>)` — detached watcher thread; `connections` is the daemon's connection-id counter, shared so herdr entries and socket connections draw from one id space.
  - Testable seam: `pub(crate) fn run_session(session: HerdrSession, registry: Registry, connections: Arc<AtomicU64>, stop: Arc<AtomicBool>, window: Option<amon_hypr::Window>)` — one session's whole life; the watcher wraps it with discovery and window resolution.

- [ ] **Step 1: Write the failing test**

In `crates/amon-daemon/src/herdr/supervisor.rs` (the fake herdr here speaks the two-connection dance: subscription connections get an ack and are held; `session.snapshot` connections get the canned snapshot):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixListener;
    use std::sync::atomic::{AtomicBool, AtomicU64};
    use std::sync::{mpsc, Arc};
    use std::time::Duration;

    use amon_protocol::AgentState;

    /// A herdr that serves one snapshot, holds subscriptions open, and hands
    /// the test a sender to push event lines into the live subscription.
    fn fake_herdr(agents: serde_json::Value) -> (tempfile::TempDir, std::path::PathBuf, mpsc::Sender<String>) {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("herdr.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let (pushes, inbox) = mpsc::channel::<String>();
        let inbox = Arc::new(std::sync::Mutex::new(inbox));
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap_or(0) == 0 {
                    continue;
                }
                let mut stream = stream;
                if line.contains("events.subscribe") {
                    let _ = writeln!(stream, r#"{{"id":"s","result":{{"type":"subscription_started"}}}}"#);
                    let inbox = inbox.clone();
                    std::thread::spawn(move || {
                        while let Ok(event) = inbox.lock().unwrap().recv() {
                            if writeln!(stream, "{event}").is_err() {
                                return;
                            }
                        }
                    });
                } else if line.contains("session.snapshot") {
                    let result = serde_json::json!({"id":"r","result":{"type":"session_snapshot","agents": agents}});
                    let _ = writeln!(stream, "{result}");
                }
            }
        });
        (dir, socket, pushes)
    }

    fn wait_for<T>(mut probe: impl FnMut() -> Option<T>) -> T {
        for _ in 0..100 {
            if let Some(value) = probe() {
                return value;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        panic!("condition not reached within 2s");
    }

    #[test]
    fn a_session_registers_its_agents_patches_status_and_deregisters_on_stop() {
        let (_dir, socket, pushes) = fake_herdr(serde_json::json!([
            {"terminal_id":"t1","agent":"claude","agent_status":"working",
             "pane_id":"w1:p1","cwd":"/repo"}
        ]));
        let registry = crate::Registry::new();
        let stop = Arc::new(AtomicBool::new(false));
        let session = crate::herdr::proc::HerdrSession {
            name: Some("work".into()),
            socket,
            client_pid: std::process::id(),
        };
        let handle = std::thread::spawn({
            let registry = registry.clone();
            let stop = stop.clone();
            move || run_session(session, registry, Arc::new(AtomicU64::new(1000)), stop, None)
        });

        // The snapshot lands as a registered entry.
        let entry = wait_for(|| registry.agents().into_iter().find(|a| a.id == "herdr:work:t1"));
        assert_eq!(entry.state, AgentState::Working);

        // A status event lands as a patch.
        pushes
            .send(r#"{"event":"pane.agent_status_changed","data":{"pane_id":"w1:p1","agent_status":"blocked"}}"#.into())
            .unwrap();
        wait_for(|| {
            registry
                .agents()
                .into_iter()
                .find(|a| a.id == "herdr:work:t1" && a.state == AgentState::Blocked)
        });

        // Stop tears every entry down.
        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        handle.join().unwrap();
        assert!(registry.agents().is_empty());
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo nextest run -p amon-daemon --status-level fail`
Expected: FAIL to compile — `run_session` does not exist.

- [ ] **Step 3: Implement**

```rust
//! One thread per attached herdr session, projecting its agents into the
//! registry for exactly as long as the session has a client (spec decision:
//! no client, no rows).
//!
//! Structure over cleverness: any structural event — an agent appearing or
//! leaving, a pane closing or moving — tears the connection down and starts
//! over with a fresh snapshot. Those are rare; re-reading the world costs a
//! few socket round trips and cannot drift. Only status changes, the common
//! case, are handled in-stream, as patches, so the registry's sound
//! machinery sees the crossings the way it does for wrapped agents.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use amon_protocol::AgentPatch;

use super::{proc, proc::HerdrSession, project, wire};
use crate::Registry;

const RESCAN_INTERVAL: Duration = Duration::from_secs(2);
const RETRY_INTERVAL: Duration = Duration::from_secs(1);

pub fn spawn(registry: Registry, connections: Arc<AtomicU64>) {
    std::thread::spawn(move || watch(registry, connections));
}

struct Live {
    stop: Arc<AtomicBool>,
    client_pid: u32,
}

fn watch(registry: Registry, connections: Arc<AtomicU64>) {
    let mut live: HashMap<Option<String>, Live> = HashMap::new();
    loop {
        let found: HashMap<Option<String>, HerdrSession> = proc::sessions()
            .into_iter()
            .map(|session| (session.name.clone(), session))
            .collect();

        // Sessions whose client is gone — or replaced — stop showing.
        live.retain(|name, session| {
            let keep = found
                .get(name)
                .is_some_and(|now| now.client_pid == session.client_pid);
            if !keep {
                session.stop.store(true, Ordering::Relaxed);
            }
            keep
        });

        for (name, session) in found {
            if live.contains_key(&name) {
                continue;
            }
            let stop = Arc::new(AtomicBool::new(false));
            live.insert(
                name,
                Live {
                    stop: stop.clone(),
                    client_pid: session.client_pid,
                },
            );
            let registry = registry.clone();
            let connections = connections.clone();
            std::thread::spawn(move || {
                let window = window_of(session.client_pid, &stop);
                run_session(session, registry, connections, stop, window);
            });
        }
        std::thread::sleep(RESCAN_INTERVAL);
    }
}

/// The client's window, waited for the way the wrapper waits for its own.
fn window_of(pid: u32, stop: &Arc<AtomicBool>) -> Option<amon_hypr::Window> {
    if stop.load(Ordering::Relaxed) {
        return None;
    }
    let directory = amon_hypr::socket_dir()?;
    amon_hypr::resolve_when_mapped(&directory, pid)
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or(0)
}

pub(crate) fn run_session(
    session: HerdrSession,
    registry: Registry,
    connections: Arc<AtomicU64>,
    stop: Arc<AtomicBool>,
    window: Option<amon_hypr::Window>,
) {
    // pane_id → (registry connection, entry id). The registry's identity is
    // the connection, so every projected agent owns one synthetic id from
    // the daemon's shared counter.
    let mut owned: HashMap<String, (u64, String)> = HashMap::new();
    let window = Arc::new(Mutex::new(window));
    spawn_window_follow(&session, &registry, &window, &stop, &owned_handle(&owned));

    while !stop.load(Ordering::Relaxed) {
        let Ok(events) = wire::subscribe(
            &session.socket,
            subscriptions_for(owned.keys().map(String::as_str)),
        ) else {
            std::thread::sleep(RETRY_INTERVAL);
            continue;
        };
        let shutdown = events.shutdown_handle();
        spawn_stop_watch(&stop, shutdown);

        let Ok(snapshot) = wire::request(&session.socket, "session.snapshot", serde_json::json!({}))
        else {
            std::thread::sleep(RETRY_INTERVAL);
            continue;
        };
        let agents = wire::agents_from(&snapshot);
        let subscribed: std::collections::HashSet<String> =
            owned.keys().cloned().collect();
        reconcile(&agents, &session, &registry, &connections, &window, &mut owned);
        // The subscription was taken out for the agent panes known *before*
        // this snapshot. If the set changed, resubscribe before trusting it.
        let current: std::collections::HashSet<String> = owned.keys().cloned().collect();
        if current != subscribed {
            continue;
        }

        for event in events {
            if stop.load(Ordering::Relaxed) {
                break;
            }
            match event.event.as_str() {
                "pane.agent_status_changed" => {
                    let Some(pane) = event.data.get("pane_id").and_then(|id| id.as_str()) else {
                        continue;
                    };
                    let Some(status) = event.data.get("agent_status").and_then(|s| s.as_str())
                    else {
                        continue;
                    };
                    let Some((connection, _)) = owned.get(pane) else {
                        continue;
                    };
                    let (state, seen) = project::state_for(status);
                    let mut patch = AgentPatch::new(String::new());
                    patch.state = Some(state);
                    patch.state_since = Some(now_ms());
                    patch.seen = Some(seen);
                    registry.update(*connection, &patch);
                }
                // Anything structural: start over with a fresh snapshot.
                _ => break,
            }
        }
    }

    for (connection, _) in owned.into_values() {
        registry.disconnect(connection);
    }
}

/// Global lifecycle events, plus one status subscription per known agent pane
/// (herdr has no global status subscription; the set is fixed per connection,
/// which is why structural changes reconnect).
fn subscriptions_for<'a>(panes: impl Iterator<Item = &'a str>) -> Vec<serde_json::Value> {
    let mut subscriptions = vec![
        serde_json::json!({"type": "pane.agent_detected"}),
        serde_json::json!({"type": "pane.closed"}),
        serde_json::json!({"type": "pane.exited"}),
        serde_json::json!({"type": "pane.moved"}),
    ];
    subscriptions.extend(
        panes.map(|pane| serde_json::json!({"type": "pane.agent_status_changed", "pane_id": pane})),
    );
    subscriptions
}

fn reconcile(
    agents: &[wire::HerdrAgent],
    session: &HerdrSession,
    registry: &Registry,
    connections: &Arc<AtomicU64>,
    window: &Arc<Mutex<Option<amon_hypr::Window>>>,
    owned: &mut HashMap<String, (u64, String)>,
) {
    let window = window
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone();
    let mut seen_panes: std::collections::HashSet<String> = Default::default();
    for agent in agents {
        seen_panes.insert(agent.pane_id.clone());
        let entry = project::entry_for(agent, session, window.as_ref(), now_ms());
        let connection = owned
            .entry(agent.pane_id.clone())
            .or_insert_with(|| (connections.fetch_add(1, Ordering::Relaxed), entry.id.clone()))
            .0;
        // Register is idempotent per connection: the same id again is an
        // update broadcast, a new one a fresh row.
        registry.register(connection, entry);
    }
    owned.retain(|pane, (connection, _)| {
        let keep = seen_panes.contains(pane);
        if !keep {
            registry.disconnect(*connection);
        }
        keep
    });
}

/// Ends a blocking event read when the session is asked to stop; the reader
/// itself cannot poll a flag while parked in `read_line`.
fn spawn_stop_watch(stop: &Arc<AtomicBool>, shutdown: wire::ShutdownHandle) {
    let stop = stop.clone();
    std::thread::spawn(move || {
        while !stop.load(Ordering::Relaxed) {
            std::thread::sleep(Duration::from_millis(200));
        }
        shutdown.shutdown();
    });
}
```

The window-follow piece (`spawn_window_follow`, `owned_handle`): the follow thread shares `owned` to patch every session agent when the client's window moves. Sharing the map means it moves behind the same mutex as the window:

Rework the two shared pieces into one `Arc<Mutex<Shared>>`:

```rust
struct Shared {
    window: Option<amon_hypr::Window>,
    /// pane_id → (registry connection, entry id)
    owned: HashMap<String, (u64, String)>,
}
```

`run_session` holds `Arc<Mutex<Shared>>`; `reconcile` and the event loop lock it briefly per operation; the follow thread does:

```rust
fn spawn_window_follow(
    session: &HerdrSession,
    registry: &Registry,
    shared: &Arc<Mutex<Shared>>,
    stop: &Arc<AtomicBool>,
) {
    let Some(directory) = amon_hypr::socket_dir() else {
        return;
    };
    let Ok(events) = amon_hypr::connect_events(&directory) else {
        return;
    };
    let Some(start) = shared
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .window
        .clone()
    else {
        return;
    };
    let pid = session.client_pid;
    let registry = registry.clone();
    let shared = shared.clone();
    let stop = stop.clone();
    std::thread::spawn(move || {
        amon_hypr::follow(&directory, events, pid, start, |window| {
            if stop.load(Ordering::Relaxed) {
                return;
            }
            let mut guard = shared.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            guard.window = window.clone();
            for (connection, _) in guard.owned.values() {
                let mut patch = AgentPatch::new(String::new());
                patch.window = Some(window.as_ref().map(|w| w.address.clone()));
                patch.workspace = Some(window.as_ref().and_then(|w| w.workspace.clone()));
                registry.update(*connection, &patch);
            }
        });
    });
}
```

(Adjust the earlier `run_session` sketch to the `Shared` shape as you implement — the test is the contract, the sketch is the map. `AgentPatch.id` is unused by `Registry::update`, which keys by connection; pass the entry id from `Shared.owned` anyway, it is free and honest.)

- [ ] **Step 4: Run tests**

Run: `cargo nextest run -p amon-daemon --status-level fail`
Expected: PASS, including the new supervisor test.

- [ ] **Step 5: Lint, then commit**

Run: `cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --all`

```bash
git add crates/amon-daemon
git commit -m "A herdr session's agents live in the registry while its client does"
```

---

### Task 7: Wire the supervisor into the daemon

**Files:**
- Modify: `crates/amon-daemon/src/lib.rs` (`run`), `crates/amon-cli/tests/e2e.rs` (pin the env guard)

**Interfaces:**
- Consumes: `herdr::spawn(registry, connections)` (Task 6).
- Produces: daemon behavior — herdr watching is on by default, off when `AMON_HERDR=0` (the guard e2e tests and doctors need for determinism on machines with a live herdr).

- [ ] **Step 1: Wire it**

In `crates/amon-daemon/src/lib.rs` `run()`: the connection counter becomes shared, and the watcher starts with the other periodic machinery:

```rust
    let shutdown = Arc::new(AtomicBool::new(false));
    let next_connection = Arc::new(AtomicU64::new(0));
    // Herdr sessions surface through the same registry as wrapped agents.
    // AMON_HERDR=0 exists for tests: an e2e daemon on a machine with a live
    // herdr must not see the developer's real agents.
    if std::env::var_os("AMON_HERDR").is_none_or(|v| v != "0") {
        herdr::spawn(registry.clone(), next_connection.clone());
    }
```

and the accept loop takes ids via `next_connection.fetch_add(1, Ordering::Relaxed)` (unchanged call, now through the Arc).

- [ ] **Step 2: Pin the e2e guard**

Open `crates/amon-cli/tests/e2e.rs`, find where the daemon process is spawned (search for `AMON_DAEMON_SOCKET`), and add `.env("AMON_HERDR", "0")` alongside it. If a shared harness function spawns the daemon, one edit covers every test.

- [ ] **Step 3: Run the whole suite**

Run: `cargo nextest run --workspace --status-level fail && cargo test --workspace --doc`
Expected: PASS, e2e included and unaffected by any herdr running on this machine.

- [ ] **Step 4: Lint**

Run: `cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --all --check`
Expected: clean.

- [ ] **Step 5: Commit**

```bash
git add crates/amon-daemon crates/amon-cli
git commit -m "The daemon watches for herdr sessions unless told not to"
```

---

### Task 8: The focus hop

`amon focus` today resolves the neediest agent's window and dispatches once. For a herdr agent the window is the herdr client's terminal — after landing there, one `agent.focus` over the session's socket selects the pane.

**Files:**
- Modify: `crates/amon-cli/src/focus.rs`
- Test: inline `#[cfg(test)]` (the fake-socket test lives here too; `amon-cli` needs `tempfile` as a dev-dependency if it lacks one)

**Interfaces:**
- Consumes: `AgentEntry.herdr: Option<HerdrInfo>` (Task 2).
- Produces: `pub(crate) fn herdr_hop(info: &HerdrInfo)` — best-effort, bounded, silent; called after a successful window dispatch.

- [ ] **Step 1: Write the failing test**

Append to `crates/amon-cli/src/focus.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixListener;

    #[test]
    fn the_hop_sends_one_agent_focus_for_the_entrys_pane() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("herdr.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let served = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            let request: serde_json::Value = serde_json::from_str(&line).unwrap();
            let mut stream = stream;
            let _ = writeln!(stream, r#"{{"id":"r","result":{{"type":"agent_focus"}}}}"#);
            request
        });

        herdr_hop(&amon_protocol::HerdrInfo {
            socket: socket.to_string_lossy().into_owned(),
            session: None,
            pane: "w1:p2".into(),
        });

        let request = served.join().unwrap();
        assert_eq!(request["method"], "agent.focus");
        assert_eq!(request["params"]["target"], "w1:p2");
    }

    #[test]
    fn a_dead_socket_is_silently_nothing() {
        // The user asked for a workspace switch; herdr being gone must not
        // turn that into an error or a hang.
        herdr_hop(&amon_protocol::HerdrInfo {
            socket: "/nonexistent/herdr.sock".into(),
            session: None,
            pane: "w1:p1".into(),
        });
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo nextest run -p amon-cli --status-level fail`
Expected: FAIL — `herdr_hop` not defined.

- [ ] **Step 3: Implement**

In `focus.rs`, rework `agent_window_on` to hand back the whole chosen entry, and hop after the window dispatch succeeds:

```rust
pub fn run(workspace: u32) -> Result<(), Box<dyn std::error::Error>> {
    if let Some(agent) = agent_on(workspace) {
        let address = agent.window.clone().unwrap_or_default();
        if dispatch(&format!(
            "hl.dsp.focus({{ window = \"address:0x{address}\" }})"
        ))? {
            // Inside herdr, the window is only half the jump: the pane the
            // agent lives in still has to come to the front.
            if let Some(herdr) = &agent.herdr {
                herdr_hop(herdr);
            }
            return Ok(());
        }
    }
    dispatch(&format!("hl.dsp.focus({{ workspace = \"{workspace}\" }})"))?;
    Ok(())
}

/// The agent on `workspace` that most wants a human, window and all.
fn agent_on(workspace: u32) -> Option<AgentEntry> {
    let mut client = Client::connect_running(RESOLVE_TIMEOUT).ok()?;
    let result = client.request(Method::Status).ok()??;
    let status: StatusResult = serde_json::from_value(result).ok()?;
    let workspace = workspace.to_string();

    status
        .agents
        .into_iter()
        .filter(|agent| agent.workspace.as_deref() == Some(workspace.as_str()))
        .filter(|agent| agent.wants_attention())
        .filter(|agent| agent.window.as_deref().is_some_and(is_address))
        .min_by_key(|agent| (agent.attention(), agent.state_since))
}

/// One `agent.focus` at herdr, then done — best-effort and bounded, because
/// this sits between a keypress and the screen settling. Every failure mode
/// leaves the user exactly where the window dispatch put them, which is
/// already the right window.
pub(crate) fn herdr_hop(info: &amon_protocol::HerdrInfo) {
    let Ok(stream) = std::os::unix::net::UnixStream::connect(&info.socket) else {
        return;
    };
    let _ = stream.set_read_timeout(Some(RESOLVE_TIMEOUT));
    let _ = stream.set_write_timeout(Some(RESOLVE_TIMEOUT));
    let Ok(mut writer) = stream.try_clone() else {
        return;
    };
    let line = serde_json::json!({
        "id": "amon-focus",
        "method": "agent.focus",
        "params": {"target": info.pane},
    });
    use std::io::{BufRead, BufReader, Write};
    if writeln!(writer, "{line}").is_err() {
        return;
    }
    // Read the reply so the request is not torn down mid-parse; what it says
    // changes nothing we could do better.
    let mut reply = String::new();
    let _ = BufReader::new(stream).read_line(&mut reply);
}
```

(Also update the doc comment on the old `agent_window_on` if any other call sites reference it — `grep -rn agent_window_on crates/`. Add `tempfile` to `crates/amon-cli/Cargo.toml` `[dev-dependencies]` if missing.)

- [ ] **Step 4: Run the whole suite and lint**

Run: `cargo nextest run --workspace --status-level fail && cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --all --check`
Expected: PASS / clean.

- [ ] **Step 5: Commit**

```bash
git add crates/amon-cli Cargo.lock
git commit -m "Focus lands on the herdr pane, not just the herdr window"
```

---

### Task 9: Live smoke test against a real herdr

Not automatable in CI; done once by hand at the end, on this machine (herdr 0.8.0 installed via mise).

- [ ] **Step 1:** `cargo build --workspace` and run the debug daemon in a terminal: `AMON_DAEMON_SOCKET=/tmp/amon-smoke.sock target/debug/amon daemon` (check `amon daemon`'s actual argv with `target/debug/amon --help` first).
- [ ] **Step 2:** In another terminal, start `herdr`, and inside it start `claude` (or any detected agent) in a pane.
- [ ] **Step 3:** `AMON_DAEMON_SOCKET=/tmp/amon-smoke.sock target/debug/amon status` — expect a `herdr:default:…` row whose state follows the agent (working while it types, blocked on a permission prompt).
- [ ] **Step 4:** Detach herdr (`ctrl+b q`) — the row must disappear within the 2 s rescan. Reattach — it returns.
- [ ] **Step 5:** With two agents in two panes on another Hyprland workspace, run `target/debug/amon focus <n>` (same socket env) and confirm it lands on the blocked agent's pane inside herdr.
- [ ] **Step 6:** Record findings (and any deviations between herdr 0.8.0's wire behavior and the 0.8.2 source this was planned against) in `docs/research/herdr-live-integration.md` under a "First live run" heading; commit that.

---

## Self-review notes

- Spec coverage: hide-without-client (watcher `retain`), first-client-wins (`sessions()` min-pid), both movement layers (window follow thread; structural-event resnapshot + `pane.moved` in the subscription set), provenance (Task 2), focus hop (Task 8). The spec's "later options" (report_agent write-back, plugin forwarder, notification routing) are deliberately out of scope.
- The supervisor's snapshot/subscription race is handled by resubscribing when the reconciled agent set differs from the subscribed one (Task 6 Step 3); events for panes not yet subscribed surface as the structural reconnect.
- Type consistency: `HerdrInfo { socket, session, pane }` is spelled identically in Tasks 2, 5, and 8; `state_for` returns `(AgentState, Option<bool>)` in Tasks 5 and 6; `run_session(session, registry, connections, stop, window)` matches its Task 6 test.
