# Runtimes Behind One Seam, and luvus — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Agents living in luvus sessions appear in amon exactly as herdr-hosted ones do, through one runtime seam in the daemon, one `runtime` field on the wire, and end-to-end tests that run the real herdr and luvus binaries in CI.

**Architecture:** `crates/amon-daemon/src/herdr/` becomes `crates/amon-daemon/src/runtime/`: a `Hosted` trait holds what differs between runtimes (process classification, socket resolution, wire bootstrap/subscription, event mapping, the focus call), while discovery, the supervisor, and projection are generic over it. `AgentEntry.herdr: Option<HerdrInfo>` becomes `AgentEntry.runtime: Option<Runtime>`, an internally tagged enum (`kind`). The wrapper steps aside on any runtime's pane env var. The e2e harness starts real herdr/luvus clients on a PTY inside the sandbox, with the binaries pinned in `.mise.toml`.

**Tech Stack:** Rust, std threads + unix sockets (no async — daemon style), serde/serde_json, portable-pty in tests, nextest, mise for tool pins.

**Spec:** `docs/superpowers/specs/2026-08-30-runtimes-and-luvus-design.md`

## Global Constraints

- Tests: `cargo nextest run --workspace --status-level fail` then `cargo test --workspace --doc` (`just test`). Never plain `cargo test`.
- Lint gate: `cargo fmt --all --check` and `cargo clippy --workspace --all-targets -- -D warnings` (`just lint`).
- After any `amon-protocol` type change: `UPDATE_SCHEMA=1 cargo nextest run -p amon-protocol --test schema` and commit `docs/protocol.schema.json`.
- `PROTOCOL_VERSION` stays 1. The `herdr` field is dropped without an alias (spec: no known readers).
- Commit messages are plain sentences in the repo's style ("The wrapper steps aside for luvus too"), no conventional-commit prefixes, and **never credit Claude or add Co-Authored-By**.
- Comments explain the constraint the code cannot show, in the repo's narrative style.
- Do not build in `/tmp`. The worktree is `~/.cache/claude-worktrees/amon-luvus`, branch `luvus`.
- luvus wire facts (verified against luvus 0.13.2 source at `/tmp/luvus`, spec §"luvus under the seam"): NDJSON `{"id","method","params"}` → `{"id","result"}`/`{"id","error"}`; `ping` → `{"version","protocol":1}`; `agent.list` → `{"type":"agent_list","agents":[{"pane":"7","agent":"claude","name":…,"status":"working","cwd":…}]}`; `events.subscribe {"after_sequence"?:N}` acks then streams `{"event","sequence","data"}`; `pane.agent_status_changed` data `{"pane","status","agent","cwd","project","branch"}`; `pane.closed` data `{"pane"}`; `pane.focus {"pane"}`; `pane.split {"direction":"right"|"down","focus":false}` → `{"pane":"N"}`; `pane.list` → `{"panes":[{"pane","agent","status","focused",…}]}`; `pane.run {"pane","command"}` types the command and Enter; `agent.report {"pane","source","agent","status":"idle|working|blocked|done"}`. Sockets: `$LUVUS_HOME`(default `~/.luvus`)`/luvus.sock` or `sessions/<name>/luvus.sock`; logical paths ≥ 100 bytes alias to `/tmp/luvus-<uid>/<fnv1a64 hex of logical path>-api.sock`. Panes get `LUVUS_ENV=1`; the server spawned by a client has `LUVUS_SESSION` in its environment for named sessions.
- herdr facts stay as in `docs/superpowers/plans/2026-08-27-herdr-live-integration.md`; the herdr CLI forms used by the e2e: `herdr --session S pane split --current --direction right --no-focus`, `herdr --session S pane run <pane_id> <command>`, `herdr --session S pane report-agent <pane_id> --source amon-e2e --agent claude --state idle|working|blocked`.

---

### Task 1: `Runtime` replaces `HerdrInfo` in the protocol

**Files:**
- Modify: `crates/amon-protocol/src/agent.rs` (the `herdr` field and `HerdrInfo`, ~lines 100–123)
- Modify: `crates/amon-protocol/src/lib.rs:26` (export)
- Modify: `crates/amon-protocol/tests/protocol.rs` (`herdr: None` at line 31, the round-trip test at ~338)
- Modify: `crates/amon-daemon/src/herdr/project.rs`, `crates/amon-daemon/src/herdr/supervisor.rs`, `crates/amon-cli/src/focus.rs` — only enough to compile (`.herdr` → `.runtime`, construct `Runtime::Herdr`); the real rework of those files is Tasks 3–5 and 7.
- Regenerate: `docs/protocol.schema.json`

**Interfaces:**
- Produces:
  ```rust
  #[serde(tag = "kind", rename_all = "lowercase")]
  pub enum Runtime {
      Herdr { socket: String, #[serde(default, skip_serializing_if = "Option::is_none")] session: Option<String>, pane: String },
      Luvus { socket: String, #[serde(default, skip_serializing_if = "Option::is_none")] session: Option<String>, pane: String },
  }
  impl Runtime {
      pub fn kind(&self) -> &'static str;            // "herdr" | "luvus"
      pub fn socket(&self) -> &str;
      pub fn pane(&self) -> &str;
      pub fn focus_request(&self) -> (&'static str, serde_json::Value);
  }
  pub struct AgentEntry { …, pub runtime: Option<Runtime> }
  ```

- [ ] **Step 1: Write the failing protocol tests**

Replace `a_herdr_entry_round_trips_and_a_plain_one_stays_bare` in `crates/amon-protocol/tests/protocol.rs` with:

```rust
#[test]
fn a_hosted_entry_round_trips_and_a_plain_one_stays_bare() {
    let hosted = AgentEntry {
        id: "luvus:abc:7".into(),
        runtime: Some(Runtime::Luvus {
            socket: "/home/mme/.luvus/luvus.sock".into(),
            session: None,
            pane: "7".into(),
        }),
        ..entry()
    };
    let json = serde_json::to_value(&hosted).unwrap();
    assert_eq!(json["runtime"]["kind"], "luvus");
    assert_eq!(json["runtime"]["pane"], "7");
    assert!(json["runtime"].get("session").is_none(), "absent, not null");
    assert!(json.get("herdr").is_none(), "the old field is gone for good");
    let back: AgentEntry = serde_json::from_value(json).unwrap();
    assert_eq!(back, hosted);

    let bare = serde_json::to_value(entry()).unwrap();
    assert!(bare.get("runtime").is_none());
}

#[test]
fn each_runtime_names_its_own_focus_call() {
    let herdr = Runtime::Herdr { socket: "/s".into(), session: Some("work".into()), pane: "w1:p2".into() };
    assert_eq!(herdr.focus_request(), ("agent.focus", serde_json::json!({"target": "w1:p2"})));
    let luvus = Runtime::Luvus { socket: "/s".into(), session: None, pane: "7".into() };
    assert_eq!(luvus.focus_request(), ("pane.focus", serde_json::json!({"pane": "7"})));
    assert_eq!(herdr.kind(), "herdr");
    assert_eq!(luvus.kind(), "luvus");
}

#[test]
fn an_unknown_runtime_kind_is_rejected_rather_than_misread() {
    let json = serde_json::json!({"kind": "zed", "socket": "/s", "pane": "1"});
    assert!(serde_json::from_value::<Runtime>(json).is_err());
}
```

`entry()` is whatever helper the file already uses to build a plain `AgentEntry` (the one with `herdr: None` at line 31 — rename that field to `runtime`). Import `Runtime` instead of `HerdrInfo`.

- [ ] **Step 2: Run to verify failure**

Run: `cargo nextest run -p amon-protocol`
Expected: compile error — `Runtime` not found.

- [ ] **Step 3: Implement**

In `crates/amon-protocol/src/agent.rs`, replace the `herdr` field and `HerdrInfo` with:

```rust
    /// Set when this agent lives inside an agent runtime (herdr, luvus)
    /// rather than under an amon wrapper.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime: Option<Runtime>,
}

/// Where a runtime-hosted agent lives. Present exactly on entries a runtime
/// module of the daemon projects; wrapped agents never carry it.
///
/// One variant per runtime, tagged by `kind`, so a consumer that only badges
/// or filters reads one field and each runtime keeps its own address shape.
/// What a variant carries is what the focus hop needs — the socket travels
/// with the entry so the CLI does not re-derive the runtime's path rules.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Runtime {
    Herdr {
        /// Absolute path of the session's JSON API socket.
        socket: String,
        /// Session name; absent for the default session.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        session: Option<String>,
        /// The pane hosting the agent (`w1:p1`) — what `agent.focus` takes.
        /// Pane ids change when panes move across herdr workspaces, so the
        /// daemon refreshes this and consumers must not cache it.
        pane: String,
    },
    Luvus {
        /// Absolute path of the session's UHP socket.
        socket: String,
        /// Session name; absent for the default session.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        session: Option<String>,
        /// The pane hosting the agent — what `pane.focus` takes. luvus pane
        /// ids are stable for the pane's life.
        pane: String,
    },
}

impl Runtime {
    pub fn kind(&self) -> &'static str {
        match self {
            Runtime::Herdr { .. } => "herdr",
            Runtime::Luvus { .. } => "luvus",
        }
    }

    pub fn socket(&self) -> &str {
        match self {
            Runtime::Herdr { socket, .. } | Runtime::Luvus { socket, .. } => socket,
        }
    }

    pub fn pane(&self) -> &str {
        match self {
            Runtime::Herdr { pane, .. } | Runtime::Luvus { pane, .. } => pane,
        }
    }

    /// The runtime's own "bring this agent to the front" call — method and
    /// params as its socket takes them. Defined once, here, so the CLI hop
    /// and any test of it agree on what the second hop is.
    pub fn focus_request(&self) -> (&'static str, serde_json::Value) {
        match self {
            Runtime::Herdr { pane, .. } => ("agent.focus", serde_json::json!({"target": pane})),
            Runtime::Luvus { pane, .. } => ("pane.focus", serde_json::json!({"pane": pane})),
        }
    }
}
```

Update `lib.rs` export: `pub use agent::{AgentEntry, AgentPatch, AgentState, Runtime};`.

Minimal compile fixes elsewhere: in `crates/amon-daemon/src/herdr/project.rs` build `runtime: Some(Runtime::Herdr { socket: …, session: …, pane: … })` and fix its tests to read `entry.runtime` (`match`/`Runtime::pane()`); in `supervisor.rs` replace `owned.entry.herdr.as_ref().map(|h| h.pane.clone())` with `owned.entry.runtime.as_ref().map(|r| r.pane().to_owned())` and the `find` in `apply_status` likewise; in `focus.rs` change `agent.herdr` → `agent.runtime` and make `herdr_hop` take `&Runtime`, sending `runtime.focus_request()` (this is the final shape for Task 7 too). Update the assertion in the supervisor test: `entry.runtime.as_ref().unwrap().pane() == "w1:p1"`.

- [ ] **Step 4: Regenerate schema and run tests**

Run: `UPDATE_SCHEMA=1 cargo nextest run -p amon-protocol --test schema && cargo nextest run --workspace --status-level fail`
Expected: PASS. `git diff docs/protocol.schema.json` shows `herdr`/`HerdrInfo` gone and a `Runtime` `oneOf` with `kind` constants.

- [ ] **Step 5: Commit**

```bash
git add crates/amon-protocol crates/amon-daemon crates/amon-cli docs/protocol.schema.json
git commit -m "The protocol names a runtime, not herdr"
```

---

### Task 2: The wrapper steps aside on any runtime's pane env

**Files:**
- Modify: `crates/amon-protocol/src/lib.rs` (the `protocol_env`-style module that holds `AMON_ENV` etc. — find it via `grep -n 'pub const AMON_ENV' crates/amon-protocol/src`)
- Modify: `crates/amon-wrapper/src/lib.rs:44-69`
- Test: `crates/amon-cli/tests/e2e.rs` (extend `inside_herdr_the_wrapper_steps_aside`)

**Interfaces:**
- Produces: `amon_protocol::env::RUNTIME_PANE_ENVS: &[&str] = &["HERDR_ENV", "LUVUS_ENV"]` (in whichever module holds the other env constants; use that module's actual path).

- [ ] **Step 1: Write the failing e2e test**

In `crates/amon-cli/tests/e2e.rs`, turn `inside_herdr_the_wrapper_steps_aside` into a shared body and two tests:

```rust
#[test]
fn inside_herdr_the_wrapper_steps_aside() {
    inside_a_runtime_pane_the_wrapper_steps_aside("HERDR_ENV");
}

#[test]
fn inside_luvus_the_wrapper_steps_aside() {
    inside_a_runtime_pane_the_wrapper_steps_aside("LUVUS_ENV");
}

/// Inside a runtime's pane the user's alias still expands `claude` to
/// `amon claude`, but wrapping there would hide the agent from the runtime
/// and give amon a row with no window. amon execs the agent bare instead;
/// the runtime detects it, and the daemon's runtime module brings it back
/// onto the bar. A wrapped agent sees AMON_ENV=1 — a bypassed one must not.
fn inside_a_runtime_pane_the_wrapper_steps_aside(pane_env: &str) {
    let sandbox = Sandbox::new();
    let agent = sandbox.fake_agent("claude", REPORTS_ITS_WRAPPING);

    let mut command = sandbox.command(&[&path_str(&agent)]);
    command.env(pane_env, "1");
    // As if the runtime had been started from inside a wrapped agent: its
    // server and every pane inherit that wrapper's routing, and this agent
    // would otherwise report its state into a wrapper watching something else.
    command.env("AMON_ENV", "1");
    command.env("AMON_AGENT_ID", "outer-agent");
    command.env("AMON_SOCKET_PATH", "/nonexistent/outer.sock");
    let output = command.output().expect("amon runs");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("amon_env=unset"), "the agent should run bare, not wrapped: {stdout:?}");
    assert!(stdout.contains("id=unset") && stdout.contains("sock=unset"), "an outer wrapper's routing must not follow the agent in: {stdout:?}");
    assert_eq!(output.status.code(), Some(7));
}
```

(`REPORTS_ITS_WRAPPING` already exists just below; move it above.) Also add `command.env_remove("LUVUS_ENV")` next to every `env_remove("HERDR_ENV")` in `harness/mod.rs` (lines ~91 and ~421).

- [ ] **Step 2: Run to verify failure**

Run: `cargo nextest run -p amon-cli --test e2e inside_luvus`
Expected: FAIL — stdout contains `amon_env=1` (wrapped).

- [ ] **Step 3: Implement**

In the protocol env module add:

```rust
/// Set by an agent runtime in every pane process it manages — `HERDR_ENV=1`,
/// `LUVUS_ENV=1`. Their contracts, not ours: the exact names each documents
/// for integrations. The wrapper steps aside when any is set, because inside
/// a runtime the runtime is the detection authority (ADR-0016, and the
/// runtime seam in the daemon).
pub const RUNTIME_PANE_ENVS: &[&str] = &["HERDR_ENV", "LUVUS_ENV"];
```

In `crates/amon-wrapper/src/lib.rs` delete `const HERDR_ENV` and replace the check:

```rust
    // Inside a runtime's pane (herdr, luvus), amon steps aside entirely. The
    // user's alias still expands `claude` to `amon claude` there, but wrapping
    // would be the worst of both worlds: the runtime sees an unrecognized
    // `amon` process and shows no agent, while the wrapper's window walk climbs
    // pane shell → runtime server → init and registers a row with nowhere to
    // jump to. Run bare instead: the runtime detects the agent natively, and
    // the daemon's runtime module carries it onto the bar with the client's
    // window — one detection authority per context
    // (docs/research/herdr-live-integration.md).
    if protocol_env::RUNTIME_PANE_ENVS
        .iter()
        .any(|name| std::env::var_os(name).is_some_and(|value| value == "1"))
    {
        return Err(exec_bare(program, args));
    }
```

- [ ] **Step 4: Run tests**

Run: `cargo nextest run -p amon-cli --test e2e steps_aside`
Expected: PASS (three tests).

- [ ] **Step 5: Commit**

```bash
git add crates/amon-protocol crates/amon-wrapper crates/amon-cli
git commit -m "The wrapper steps aside for luvus too"
```

---

### Task 3: The seam — `runtime/` module with `Hosted`, generic wire, generic discovery

Pure move-and-generalize; behaviour for herdr unchanged. After this task the herdr implementation lives in `runtime/herdr/`, and the supervisor/projection are generic.

**Files:**
- Create: `crates/amon-daemon/src/runtime/mod.rs`, `runtime/wire.rs`, `runtime/discover.rs`, `runtime/herdr/mod.rs`, `runtime/herdr/proc.rs`, `runtime/herdr/wire.rs`
- Move + modify: `herdr/supervisor.rs` → `runtime/supervisor.rs`, `herdr/project.rs` → `runtime/project.rs`
- Delete: `crates/amon-daemon/src/herdr/`
- Modify: `crates/amon-daemon/src/lib.rs:75-80`

**Interfaces (Produces):**

`runtime/mod.rs`:
```rust
pub mod discover; pub mod project; mod supervisor; pub mod wire;
pub mod herdr;

pub use supervisor::spawn;   // spawn(registry, connections) starts one watcher per runtime

/// One session of a runtime with an attached client.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Session { pub name: Option<String>, pub socket: PathBuf, pub client_pid: u32 }

pub enum Role { Client { name: Option<String> }, Server { name: Option<String> }, Other }

/// An agent as the runtime reports it, in the shape the projection needs.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct HostedAgent {
    /// Stable within one life of the agent: herdr's terminal_id, luvus's pane.
    pub identity: String,
    pub pane: String,
    pub agent: Option<String>,
    pub status: String,
    pub cwd: Option<String>,
    /// A per-life counter (herdr's state_change_seq); None where the runtime has none.
    pub life: Option<u64>,
}

pub enum HostedEvent {
    /// `agent`: None = the event did not say; Some(None) = the runtime has no name for it now.
    Status { pane: String, status: String, agent: Option<Option<String>> },
    /// Anything that changes the set of agents or panes: resnapshot.
    Structural,
    Ignore,
}

/// What differs between runtimes. Everything else — discovery, the
/// supervisor, projection — is generic over this.
pub trait Hosted: Send + 'static {
    const KIND: &'static str;      // id prefix and Runtime variant name
    const COMM: &'static str;      // process comm of client and server
    /// Whether status events are subscribed per pane (a change to the pane set re-attaches).
    const STATUS_PER_PANE: bool;

    fn classify(argv: &[String], is_session_leader: bool, has_tty: bool, environ: &str) -> Role;
    fn socket_for(argv: &[String], environ: &str) -> Option<PathBuf>;

    fn new() -> Self;
    /// Subscribe (given the panes currently owned), then snapshot. Returns the agents now and the stream.
    fn attach(&mut self, socket: &Path, panes: &[String]) -> std::io::Result<(Vec<HostedAgent>, wire::EventStream)>;
    /// Classify one pushed event, given pane → agent label for owned panes.
    fn event(&mut self, event: &wire::RawEvent, owned: &HashMap<String, String>) -> HostedEvent;
    fn runtime(session: &Session, agent: &HostedAgent) -> amon_protocol::Runtime;
}
```

`runtime/wire.rs` (the old `herdr/wire.rs` minus `HerdrAgent`/`agents_from`; `HerdrEvent` → `RawEvent { event: String, sequence: Option<u64>, data: Value }`):
```rust
pub fn request(socket: &Path, method: &str, params: Value) -> io::Result<Value>;
pub fn subscribe(socket: &Path, params: Value) -> io::Result<EventStream>;   // params is the whole `params` object
pub struct EventStream; impl Iterator<Item = RawEvent>; fn shutdown_handle(&self) -> ShutdownHandle;
```

`runtime/discover.rs`:
```rust
pub fn sessions<H: Hosted>() -> Vec<Session>;   // the old proc::sessions, comm == H::COMM, classify/socket_for via H
pub fn env_value<'a>(environ: &'a str, key: &str) -> Option<&'a str>;
pub fn stat_fields(stat: &str) -> Option<(u32, u32)>;
/// Only sessions whose socket lies under this directory are adopted, when set.
pub const ROOT_ENV: &str = "AMON_RUNTIMES_ROOT";
```

`runtime/project.rs`: `entry_for<H: Hosted>(agent: &HostedAgent, session: &Session, window: Option<&Window>, now_ms: u64) -> AgentEntry` (id `format!("{}:{}:{}", H::KIND, socket_digest, agent.identity)`, `runtime: Some(H::runtime(session, agent))`), `state_for`, `UNNAMED` unchanged.

`runtime/herdr/`: `proc.rs` = old classify/socket_for/etc. (tests included); `wire.rs` = `HerdrAgent` + `agents_from` + `impl From<HerdrAgent> for HostedAgent`; `mod.rs`:
```rust
pub struct Herdr;
impl Hosted for Herdr {
    const KIND: &'static str = "herdr"; const COMM: &'static str = "herdr"; const STATUS_PER_PANE: bool = true;
    fn classify(..) = proc::classify(..); fn socket_for(..) = proc::socket_for(..);
    fn new() -> Self { Herdr }
    fn attach(&mut self, socket, panes) {
        let events = wire::subscribe(socket, json!({"subscriptions": subscriptions_for(panes)}))?;  // moved from supervisor
        let snapshot = wire::request(socket, "session.snapshot", json!({}))?;
        Ok((agents_from(&snapshot).into_iter().map(Into::into).collect(), events))
    }
    fn event(&mut self, event, _owned) -> HostedEvent {
        if event.event != "pane.agent_status_changed" { return HostedEvent::Structural; }
        // pane_id + agent_status required; agent: absent → None, null → Some(None), string → Some(Some)
        …
    }
    fn runtime(session, agent) = Runtime::Herdr { socket: session.socket.to_string_lossy().into_owned(), session: session.name.clone(), pane: agent.pane.clone() }
}
```

Supervisor changes: `watch::<H>` / `run_session::<H>`; the attach+resnapshot loop becomes

```rust
let mut host = H::new();
while !stop.load(Ordering::Relaxed) {
    let panes: Vec<String> = owned panes…;
    let Ok((agents, events)) = host.attach(&session.socket, &panes) else { sleep(RETRY_INTERVAL); continue };
    *lock(&current) = Some(events.shutdown_handle());
    if stop.load(..) { break; }
    reconcile::<H>(&agents, &session, &registry, &connections, &shared);
    if H::STATUS_PER_PANE && pane set changed { continue; }
    for event in events {
        if stop.load(..) { break; }
        let owned_labels: HashMap<String, String> = lock(&shared).owned.values().map(|o| (o.entry.runtime.pane().to_owned(), o.entry.agent.clone())).collect();
        match host.event(&event, &owned_labels) {
            HostedEvent::Status { pane, status, agent } => apply_status(&registry, &shared, &pane, &status, agent),
            HostedEvent::Structural => break,
            HostedEvent::Ignore => {}
        }
    }
}
```

`apply_status` takes the decomposed fields instead of raw JSON. `reconcile` keys `owned` by `agent.identity` and uses `agent.life` in `restarted`. `Owned.seq` → `Owned.life`.

`spawn` in `supervisor.rs`:
```rust
pub fn spawn(registry: Registry, connections: Arc<AtomicU64>) {
    std::thread::spawn({ let (r, c) = (registry.clone(), connections.clone()); move || watch::<herdr::Herdr>(r, c) });
}
```
(luvus is added in Task 4.)

`discover::sessions` gains, after `socket_for`: if `std::env::var_os(ROOT_ENV)` is set and `!socket.starts_with(root)`, skip — with a comment: *an e2e daemon on a developer's desktop must not adopt the developer's live sessions; the sandbox names the only root it wants seen.*

`lib.rs`: `mod runtime;` and

```rust
    // Agents inside runtime sessions (herdr, luvus) surface through the same
    // registry as wrapped ones. AMON_RUNTIMES=0 exists for tests: an e2e
    // daemon on a machine with a live session must not see the developer's
    // real agents.
    if std::env::var_os("AMON_RUNTIMES").is_none_or(|value| value != "0") {
        runtime::spawn(registry.clone(), next_connection.clone());
    }
```

Rename `AMON_HERDR` → `AMON_RUNTIMES` in `harness/mod.rs` (2 places) and `e2e.rs:2776`.

- [ ] **Step 1: Move files with git so history follows**

```bash
cd ~/.cache/claude-worktrees/amon-luvus
git mv crates/amon-daemon/src/herdr crates/amon-daemon/src/runtime
mkdir crates/amon-daemon/src/runtime/herdr
git mv crates/amon-daemon/src/runtime/proc.rs crates/amon-daemon/src/runtime/herdr/proc.rs
git mv crates/amon-daemon/src/runtime/wire.rs crates/amon-daemon/src/runtime/herdr/wire.rs
```
Then create `runtime/wire.rs` by extracting `request`/`subscribe`/`EventStream`/`ShutdownHandle` (and their tests: `a_request_returns_the_result_value`, `an_error_response_is_an_error`, `a_request_names_its_method_and_params_on_the_wire`, `a_subscription_acks_then_streams_events`, `shutdown_unblocks_a_waiting_reader`) out of `herdr/wire.rs`, leaving `HerdrAgent`, `agents_from` and their tests there. Create `runtime/discover.rs` by extracting `sessions`, `env_value`, `stat_fields`, `claim` from `herdr/proc.rs` (test `the_stat_line_yields_sid_and_tty_past_a_hostile_comm` moves with `stat_fields`; `two_clients_on_one_socket_are_one_session_and_the_first_wins` moves with `claim`).

- [ ] **Step 2: Write the failing tests for the new generic pieces**

In `runtime/discover.rs`:
```rust
#[test]
fn a_root_limits_which_sockets_are_adopted() {
    assert!(under_root(Path::new("/tmp/sb/home/.config/herdr/herdr.sock"), Some(Path::new("/tmp/sb"))));
    assert!(!under_root(Path::new("/home/me/.config/herdr/herdr.sock"), Some(Path::new("/tmp/sb"))));
    assert!(under_root(Path::new("/home/me/.config/herdr/herdr.sock"), None), "no root: everything");
}
```
with `fn under_root(socket: &Path, root: Option<&Path>) -> bool` used by `sessions`.

In `runtime/herdr/mod.rs`:
```rust
#[test]
fn herdr_events_split_into_status_and_structural() {
    let mut host = Herdr::new();
    let owned = HashMap::new();
    let status = RawEvent { event: "pane.agent_status_changed".into(), sequence: None,
        data: json!({"pane_id": "w1:p1", "agent_status": "blocked", "agent": null}) };
    assert!(matches!(host.event(&status, &owned), HostedEvent::Status { ref pane, ref status, agent: Some(None) } if pane == "w1:p1" && status == "blocked"));
    let closed = RawEvent { event: "pane.closed".into(), sequence: None, data: json!({}) };
    assert!(matches!(host.event(&closed, &owned), HostedEvent::Structural));
}

#[test]
fn a_herdr_agent_becomes_a_hosted_agent_keyed_by_terminal() {
    let hosted: HostedAgent = HerdrAgent { terminal_id: "t1".into(), agent_status: "done".into(), pane_id: "w1:p2".into(), agent: Some("claude".into()), cwd: None, state_change_seq: Some(3) }.into();
    assert_eq!(hosted.identity, "t1"); assert_eq!(hosted.pane, "w1:p2"); assert_eq!(hosted.life, Some(3));
}
```

The existing supervisor and project tests are kept, adapted to the generic signatures (`run_session::<Herdr>`, `entry_for::<Herdr>`, `Session` instead of `HerdrSession`, `HostedAgent` instead of `HerdrAgent` in `project` tests).

- [ ] **Step 3: Run to verify failure**

Run: `cargo nextest run -p amon-daemon`
Expected: compile errors for the new items.

- [ ] **Step 4: Implement the seam as specified in Interfaces**

Follow the interface block above exactly. Keep every existing comment that still holds; reword "herdr" to "the runtime" only where the sentence is now about both.

- [ ] **Step 5: Run everything**

Run: `just test && just lint`
Expected: PASS. `cargo nextest run -p amon-daemon` lists the moved tests under `runtime::…`.

- [ ] **Step 6: Commit**

```bash
git add -A crates/amon-daemon crates/amon-cli
git commit -m "Runtimes live behind one seam, herdr first"
```

---

### Task 4: luvus behind the seam

**Files:**
- Create: `crates/amon-daemon/src/runtime/luvus/mod.rs`, `runtime/luvus/proc.rs`, `runtime/luvus/wire.rs`
- Modify: `crates/amon-daemon/src/runtime/mod.rs` (`pub mod luvus;`), `runtime/supervisor.rs::spawn` (second watcher)

**Interfaces:**
- Consumes: everything from Task 3.
- Produces: `pub struct Luvus { after: Option<u64> }` implementing `Hosted` with `KIND = "luvus"`, `COMM = "luvus"`, `STATUS_PER_PANE = false`.

- [ ] **Step 1: Write the failing tests**

`runtime/luvus/proc.rs` tests (mirror the herdr ones):

```rust
fn argv(parts: &[&str]) -> Vec<String> { parts.iter().map(|p| (*p).to_owned()).collect() }

#[test]
fn a_bare_luvus_with_a_tty_is_a_client_of_the_default_session() {
    assert_eq!(classify(&argv(&["luvus"]), true, true, ""), Role::Client { name: None });
}

#[test]
fn the_session_flag_names_the_session_in_every_spelling() {
    for args in [&["luvus", "--session", "work"][..], &["luvus", "--session=work"], &["luvus", "session", "attach", "work"]] {
        assert_eq!(classify(&argv(args), true, true, ""), Role::Client { name: Some("work".into()) }, "{args:?}");
    }
}

#[test]
fn attach_client_and_local_are_clients_too() {
    for args in [&["luvus", "attach", "7"][..], &["luvus", "client"], &["luvus", "--local"]] {
        assert!(matches!(classify(&argv(args), true, true, ""), Role::Client { .. }), "{args:?}");
    }
}

#[test]
fn the_server_is_a_server_and_names_its_session_from_environ() {
    assert_eq!(classify(&argv(&["luvus", "server"]), true, false, "LUVUS_SESSION=work\0"), Role::Server { name: Some("work".into()) });
}

#[test]
fn remote_forms_and_cli_subcommands_are_neither() {
    for args in [&["luvus", "--remote", "box"][..], &["luvus", "remote-client-bridge"], &["luvus", "agent", "list"], &["luvus", "--version"], &["luvus", "doctor"], &["luvus", "uhp", "schema"]] {
        assert_eq!(classify(&argv(args), true, true, ""), Role::Other, "{args:?}");
    }
    assert_eq!(classify(&argv(&["luvus"]), true, false, ""), Role::Other, "no tty, no client");
}

#[test]
fn the_socket_follows_luvus_home_and_the_session() {
    assert_eq!(socket_for(&argv(&["luvus"]), "HOME=/home/me\0"), Some(PathBuf::from("/home/me/.luvus/luvus.sock")));
    assert_eq!(socket_for(&argv(&["luvus", "--session", "work"]), "HOME=/home/me\0LUVUS_HOME=/x/luvus\0"), Some(PathBuf::from("/x/luvus/sessions/work/luvus.sock")));
    assert_eq!(socket_for(&argv(&["luvus"]), "HOME=/home/me\0LUVUS_SESSION=work\0"), Some(PathBuf::from("/home/me/.luvus/sessions/work/luvus.sock")));
}

#[test]
fn a_long_home_aliases_into_tmp_the_way_luvus_does() {
    let home = format!("/home/{}", "x".repeat(120));
    let socket = socket_for(&argv(&["luvus"]), &format!("HOME={home}\0")).unwrap();
    assert!(socket.starts_with("/tmp/luvus-"), "{socket:?}");
    assert!(socket.to_string_lossy().ends_with("-api.sock"));
    assert!(socket.as_os_str().len() < 100);
}
```

`runtime/luvus/mod.rs` tests:

```rust
#[test]
fn a_status_event_for_an_owned_pane_with_the_same_agent_is_a_status() {
    let mut host = Luvus::new();
    let owned = HashMap::from([("7".to_string(), "claude".to_string())]);
    let event = RawEvent { event: "pane.agent_status_changed".into(), sequence: Some(41),
        data: json!({"pane": "7", "status": "blocked", "agent": "claude"}) };
    assert!(matches!(host.event(&event, &owned), HostedEvent::Status { ref pane, ref status, agent: Some(Some(ref a)) } if pane == "7" && status == "blocked" && a == "claude"));
    assert_eq!(host.after, Some(41), "the cursor follows every event");
}

#[test]
fn a_status_event_for_a_new_pane_or_a_changed_agent_is_structural() {
    let mut host = Luvus::new();
    let owned = HashMap::from([("7".to_string(), "claude".to_string())]);
    let unknown = RawEvent { event: "pane.agent_status_changed".into(), sequence: Some(1), data: json!({"pane": "8", "status": "working", "agent": "codex"}) };
    assert!(matches!(host.event(&unknown, &owned), HostedEvent::Structural));
    let exited = RawEvent { event: "pane.agent_status_changed".into(), sequence: Some(2), data: json!({"pane": "7", "status": "idle", "agent": ""}) };
    assert!(matches!(host.event(&exited, &owned), HostedEvent::Structural));
}

#[test]
fn pane_lifecycle_and_resync_are_structural_and_the_rest_is_noise() {
    let mut host = Luvus::new();
    let owned = HashMap::new();
    for name in ["pane.closed", "pane.created", "pane.moved", "events.resync_required"] {
        let event = RawEvent { event: name.into(), sequence: None, data: json!({}) };
        assert!(matches!(host.event(&event, &owned), HostedEvent::Structural), "{name}");
    }
    let frame = RawEvent { event: "terminal.frame".into(), sequence: None, data: json!({}) };
    assert!(matches!(host.event(&frame, &owned), HostedEvent::Ignore));
}

#[test]
fn agent_list_records_become_hosted_agents_keyed_by_pane() {
    let result = json!({"type": "agent_list", "agents": [
        {"pane": "7", "agent": "claude", "name": null, "status": "done", "cwd": "/repo"},
        {"pane": "9", "agent": "codex"}   // no status: skipped, never guessed
    ]});
    let agents = wire::agents_from(&result);
    assert_eq!(agents.len(), 1);
    assert_eq!(agents[0], HostedAgent { identity: "7".into(), pane: "7".into(), agent: Some("claude".into()), status: "done".into(), cwd: Some("/repo".into()), life: None });
}

#[test]
fn attach_pings_for_protocol_one_subscribes_after_the_cursor_and_lists_agents() {
    // A fake luvus that records request lines; reuse the fake-server helper pattern from runtime/wire.rs tests.
    let (dir, socket, requests) = fake_luvus(vec![
        r#"{"id":"x","result":{"type":"pong","version":"0.13.2","protocol":1}}"#,
        r#"{"id":"x","result":{"type":"subscription_started","sequence":41}}"#,
        r#"{"id":"x","result":{"type":"agent_list","agents":[]}}"#,
    ]);
    let mut host = Luvus { after: Some(40) };
    let (agents, _events) = host.attach(&socket, &[]).unwrap();
    assert!(agents.is_empty());
    let seen = requests.lock().unwrap().join("\n");
    assert!(seen.contains(r#""method":"ping""#));
    assert!(seen.contains(r#""after_sequence":40"#));
    assert!(seen.contains(r#""method":"agent.list""#));
    drop(dir);
}

#[test]
fn a_foreign_protocol_is_refused_before_anything_is_projected() {
    let (_dir, socket, _requests) = fake_luvus(vec![r#"{"id":"x","result":{"type":"pong","version":"9.0.0","protocol":2}}"#]);
    assert!(Luvus::new().attach(&socket, &[]).is_err());
}
```

(`fake_luvus` answers each accepted connection's first line with the next canned reply and records every request line; write it in the luvus test module — ~30 lines, modelled on `runtime/wire.rs`'s test fake.)

- [ ] **Step 2: Run to verify failure**

Run: `cargo nextest run -p amon-daemon luvus`
Expected: compile errors.

- [ ] **Step 3: Implement**

`runtime/luvus/proc.rs`:

```rust
//! Which `luvus` processes are attached clients, and which socket they use.
//! luvus's process model matches herdr's: the process a user types is the
//! tty-owning TUI client, `luvus server` is spawned detached under setsid.

use std::path::{Path, PathBuf};
use crate::runtime::discover::env_value;
use crate::runtime::Role;

const VALUE_OPTIONS: &[&str] = &["--session", "--remote"];
const PRINT_AND_EXIT: &[&str] = &["--version", "-V", "--help", "-h"];

pub fn classify(argv: &[String], _is_session_leader: bool, has_tty: bool, environ: &str) -> Role {
    let args: Vec<&str> = argv.iter().skip(1).map(String::as_str).collect();
    let positional = positionals(&args);
    if positional.first() == Some(&"server") {
        return Role::Server { name: environ_session(environ) };
    }
    if !has_tty { return Role::Other; }
    if args.iter().any(|arg| *arg == "--remote" || arg.starts_with("--remote=")) { return Role::Other; }
    if args.iter().any(|arg| PRINT_AND_EXIT.contains(arg)) { return Role::Other; }
    // `luvus`, `luvus --session X`, `luvus --local` attach the TUI in-process;
    // so do `attach <id>`, `client`, and `session attach X`. Every other
    // subcommand is the CLI: one request and out.
    let is_client = match positional.split_first() {
        None => true,
        Some((&"client", _)) | Some((&"attach", _)) => true,
        Some((&"session", rest)) => rest.first() == Some(&"attach"),
        Some(_) => false,
    };
    if is_client { Role::Client { name: argv_session(&args).or_else(|| environ_session(environ)) } } else { Role::Other }
}
```

`positionals`, `argv_session` (also reading `session attach <name>`), `environ_session` (`LUVUS_SESSION`) as in the herdr version. `socket_for`:

```rust
pub fn socket_for(argv: &[String], environ: &str) -> Option<PathBuf> {
    let args: Vec<&str> = argv.iter().skip(1).map(String::as_str).collect();
    let name = argv_session(&args).or_else(|| environ_session(environ));
    let home = luvus_home(environ)?;
    let logical = match name.as_deref() {
        None => home.join("luvus.sock"),
        Some(name) => home.join("sessions").join(name).join("luvus.sock"),
    };
    Some(alias(logical))
}

/// `$LUVUS_HOME`, else `~/.luvus` for the *client's* environment, falling
/// back to the daemon's own HOME when the environ could not be read.
fn luvus_home(environ: &str) -> Option<PathBuf> {
    if let Some(home) = env_value(environ, "LUVUS_HOME") { return Some(PathBuf::from(home)); }
    env_value(environ, "HOME").map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(PathBuf::from))
        .map(|home| home.join(".luvus"))
}

/// luvus keeps sockets under the home unless the path would not fit
/// `sun_path` (macOS's 104-byte limit, applied everywhere), in which case it
/// hashes the logical path into `/tmp/luvus-<uid>/`. Same hash, same
/// threshold, or a long home is a session amon never finds.
fn alias(logical: PathBuf) -> PathBuf {
    use std::os::unix::ffi::OsStrExt;
    let bytes = logical.as_os_str().as_bytes();
    if bytes.len() < 100 { return logical; }
    let mut hash = 0xcbf29ce484222325u64;
    for byte in bytes { hash ^= u64::from(*byte); hash = hash.wrapping_mul(0x100000001b3); }
    let uid = unsafe { libc::geteuid() };
    PathBuf::from(format!("/tmp/luvus-{uid}/{hash:016x}-api.sock"))
}
```

`runtime/luvus/wire.rs`:

```rust
#[derive(serde::Deserialize)]
struct Record { pane: String, status: String, #[serde(default)] agent: Option<String>, #[serde(default)] cwd: Option<String> }

pub fn agents_from(result: &Value) -> Vec<HostedAgent> {
    result.get("agents").and_then(Value::as_array).map(|agents| agents.iter()
        .filter_map(|a| serde_json::from_value::<Record>(a.clone()).ok())
        .map(|r| HostedAgent { identity: r.pane.clone(), pane: r.pane, agent: r.agent.filter(|a| !a.is_empty()), status: r.status, cwd: r.cwd, life: None })
        .collect()).unwrap_or_default()
}
```

`runtime/luvus/mod.rs`:

```rust
pub struct Luvus { pub(crate) after: Option<u64> }

impl Hosted for Luvus {
    const KIND: &'static str = "luvus";
    const COMM: &'static str = "luvus";
    const STATUS_PER_PANE: bool = false;
    fn classify(..) { proc::classify(..) }  fn socket_for(..) { proc::socket_for(..) }
    fn new() -> Self { Luvus { after: None } }

    fn attach(&mut self, socket: &Path, _panes: &[String]) -> io::Result<(Vec<HostedAgent>, EventStream)> {
        // UHP promises additive growth within a major; a different protocol
        // number is a different contract, and projecting from it would be
        // guessing.
        let pong = wire::request(socket, "ping", json!({}))?;
        if pong.get("protocol").and_then(Value::as_u64) != Some(1) {
            return Err(io::Error::other(format!("luvus protocol {} is not 1", pong["protocol"])));
        }
        // Global and sequenced: one subscription covers every pane, and the
        // cursor means a reconnect replays what the blip would have lost.
        let params = match self.after { Some(after) => json!({"after_sequence": after}), None => json!({}) };
        let events = wire::subscribe(socket, params)?;
        let list = wire::request(socket, "agent.list", json!({}))?;
        Ok((wire::agents_from(&list), events))
    }

    fn event(&mut self, event: &RawEvent, owned: &HashMap<String, String>) -> HostedEvent {
        if let Some(sequence) = event.sequence { self.after = Some(sequence); }
        match event.event.as_str() {
            "pane.agent_status_changed" => {
                let (Some(pane), Some(status)) = (event.data.get("pane").and_then(Value::as_str), event.data.get("status").and_then(Value::as_str)) else { return HostedEvent::Ignore };
                let agent = event.data.get("agent").and_then(Value::as_str).unwrap_or("");
                // luvus has no "agent appeared/left" event: the pane's agent
                // label changing — from nothing, to nothing, or to another —
                // is that moment, and a snapshot is the honest answer to it.
                match owned.get(pane) {
                    Some(label) if label == agent => HostedEvent::Status { pane: pane.into(), status: status.into(), agent: Some(Some(agent.into())) },
                    _ => HostedEvent::Structural,
                }
            }
            "pane.closed" | "pane.created" | "pane.moved" | "events.resync_required" => HostedEvent::Structural,
            _ => HostedEvent::Ignore,
        }
    }

    fn runtime(session: &Session, agent: &HostedAgent) -> Runtime {
        Runtime::Luvus { socket: session.socket.to_string_lossy().into_owned(), session: session.name.clone(), pane: agent.pane.clone() }
    }
}
```

`wire::subscribe` must populate `RawEvent.sequence` from the `sequence` field when present. Add to `spawn`: a second thread `watch::<luvus::Luvus>`.

- [ ] **Step 4: Run tests and lint**

Run: `cargo nextest run -p amon-daemon && just lint`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/amon-daemon
git commit -m "luvus is the second runtime"
```

---

### Task 5: `amon focus` hops for window-less hosted agents

**Files:**
- Modify: `crates/amon-cli/src/focus.rs:71-97` (`go_to`), and `herdr_hop` → `runtime_hop(&Runtime)`
- Test: unit test in `focus.rs` against a fake socket (the module already has none; add a `#[cfg(test)] mod tests`)

- [ ] **Step 1: Write the failing test**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixListener;

    /// A socket that records the one request it gets and answers ok.
    fn fake_runtime() -> (tempfile::TempDir, std::path::PathBuf, std::sync::Arc<std::sync::Mutex<Vec<String>>>) {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("rt.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let log = seen.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let mut line = String::new();
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                if reader.read_line(&mut line).unwrap_or(0) > 0 {
                    log.lock().unwrap().push(line);
                    let mut stream = stream;
                    let _ = writeln!(stream, r#"{{"id":"amon-focus","result":{{"type":"ok"}}}}"#);
                }
            }
        });
        (dir, socket, seen)
    }

    #[test]
    fn a_hosted_agent_without_a_window_still_gets_its_hop() {
        let (_dir, socket, seen) = fake_runtime();
        let agent = AgentEntry {
            window: None,
            runtime: Some(Runtime::Luvus { socket: socket.to_string_lossy().into_owned(), session: None, pane: "7".into() }),
            ..plain_entry()
        };
        assert!(go_to(&agent).unwrap(), "the hop is the whole jump here");
        let line = seen.lock().unwrap().join("");
        assert!(line.contains(r#""method":"pane.focus""#) && line.contains(r#""pane":"7""#), "{line}");
    }

    #[test]
    fn a_plain_agent_without_a_window_is_nothing_to_jump_to() {
        assert!(!go_to(&AgentEntry { window: None, runtime: None, ..plain_entry() }).unwrap());
    }
}
```

`plain_entry()` builds an `AgentEntry` with every field defaulted (`id: "x"`, `agent: "claude"`, `state: AgentState::Idle`, zeros/None/empty elsewhere). Add `tempfile` to `[dev-dependencies]` of `amon-cli` if absent.

- [ ] **Step 2: Run to verify failure**

Run: `cargo nextest run -p amon-cli --lib focus`
Expected: FAIL — `go_to` returns false for the window-less hosted agent.

- [ ] **Step 3: Implement**

```rust
fn go_to(agent: &AgentEntry) -> Result<bool, Box<dyn std::error::Error>> {
    // Checked here rather than trusted from the caller: this interpolates
    // into a Lua expression, and the token arrives over a socket.
    let address = agent.window.as_deref().filter(|window| is_address(window));
    let moved = match address {
        Some(address) => dispatch(&format!("hl.dsp.focus({{ window = \"address:0x{address}\" }})"))?,
        None => false,
    };
    // A window that was there and is not now (closed in between, or the
    // compositor never knew it) ends the jump: the pane hop marks the agent
    // seen, and an agent nobody was taken to has not been seen.
    if address.is_some() && !moved {
        return Ok(false);
    }
    // Inside a runtime the window is only half the jump: every agent in a
    // session shares the client's terminal, and this one's pane still has to
    // come to the front. With no window at all — over ssh, no compositor —
    // the hop is the whole jump, and the only part of it amon can make.
    let Some(runtime) = &agent.runtime else {
        return Ok(moved);
    };
    runtime_hop(runtime);
    Ok(true)
}

/// One focus request at the runtime, then done — best-effort and bounded,
/// because this sits between a keypress and the screen settling. Every
/// failure mode leaves the user where the window dispatch put them.
fn runtime_hop(runtime: &Runtime) {
    use std::io::{BufRead, BufReader, Write};
    let Ok(stream) = std::os::unix::net::UnixStream::connect(runtime.socket()) else { return };
    let _ = stream.set_read_timeout(Some(RESOLVE_TIMEOUT));
    let _ = stream.set_write_timeout(Some(RESOLVE_TIMEOUT));
    let Ok(mut writer) = stream.try_clone() else { return };
    let (method, params) = runtime.focus_request();
    let line = serde_json::json!({"id": "amon-focus", "method": method, "params": params});
    if writeln!(writer, "{line}").is_err() { return; }
    let mut reply = String::new();
    let _ = BufReader::new(stream).read_line(&mut reply);
}
```

Update the module doc's mention of "the herdr pane hop" to "the runtime's pane hop". `run(workspace)` is untouched: `agent_on` only returns agents with a window.

- [ ] **Step 4: Run tests**

Run: `cargo nextest run -p amon-cli && just lint`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/amon-cli
git commit -m "A hosted agent without a window still gets its pane hop"
```

---

### Task 6: Pin the runtimes and teach the harness to start them

**Files:**
- Modify: `.mise.toml`
- Modify: `.github/workflows/ci.yml` and `publish.yml` — no new steps (mise-action installs pins); update the socat comment to say the runtimes come from mise too
- Create: `crates/amon-cli/tests/harness/hosted.rs`
- Modify: `crates/amon-cli/tests/harness/mod.rs` (`pub mod hosted;`, `PtySession::start_program`, `AMON_RUNTIMES_ROOT`)

**Interfaces (Produces):**

```rust
// harness/hosted.rs
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind { Herdr, Luvus }
impl Kind { pub fn name(self) -> &'static str /* "herdr"|"luvus" */; pub fn all() -> [Kind; 2]; }

pub struct Hosted { pub kind: Kind, pub session: String, pub socket: PathBuf, client: PtySession, sandbox_home: PathBuf, … }
impl Hosted {
    /// Starts `<runtime> --session amon-e2e` on a PTY inside the sandbox; waits for the API socket.
    pub fn start(sandbox: &Sandbox, kind: Kind) -> Self;
    /// Splits off an unfocused pane and types `command` + Enter into it; returns the pane id.
    pub fn run_unfocused(&self, command: &str) -> String;
    /// The runtime's own report call for `pane`.
    pub fn report(&self, pane: &str, status: &str);
    /// `agent.list` as the runtime sees it: (pane, status) pairs.
    pub fn agents(&self) -> Vec<(String, String)>;
    pub fn client_pid(&self) -> u32;
    pub fn detach(&mut self);           // kills the client; the server keeps running
    pub fn reattach(&mut self, sandbox: &Sandbox);
    pub fn request(&self, method: &str, params: serde_json::Value) -> serde_json::Value;  // raw NDJSON, panics on error
}
```

`Sandbox::command` and `PtySession::start` set `AMON_RUNTIMES_ROOT` to the sandbox runtime dir and — for tests that opt in via `Sandbox::with_runtimes()` — `AMON_RUNTIMES=1`; the default stays `0`.

- [ ] **Step 1: Pin the binaries**

`.mise.toml`:
```toml
# The runtimes the e2e tests drive for real. Pinned, so CI and a developer run
# the same binaries, and a bump is a reviewed diff whose CI run is the
# compatibility check against that release's socket protocol.
"ubi:ogulcancelik/herdr" = "0.8.2"
"ubi:RizRiyz/luvus" = "0.13.2"
```
Run `mise install` in the worktree and confirm `mise x -- luvus --version` prints 0.13.2 and `mise x -- herdr --version` 0.8.2. (If ubi cannot pick the linux-musl tarball for luvus, add `[tools."ubi:RizRiyz/luvus"] matching = "linux-musl"` per mise's ubi backend options.)

- [ ] **Step 2: Write the failing harness smoke test**

In `crates/amon-cli/tests/e2e.rs`:

```rust
mod hosted_runtimes {
    use super::*;
    use harness::hosted::{Hosted, Kind};

    #[test] fn herdr_starts_and_lists_a_fake_agent() { a_runtime_starts_and_lists_a_fake_agent(Kind::Herdr) }
    #[test] fn luvus_starts_and_lists_a_fake_agent() { a_runtime_starts_and_lists_a_fake_agent(Kind::Luvus) }

    fn a_runtime_starts_and_lists_a_fake_agent(kind: Kind) {
        let sandbox = Sandbox::new();
        let agent = sandbox.fake_agent("claude", WORKING_THEN_IDLE);
        let hosted = Hosted::start(&sandbox, kind);
        let pane = hosted.run_unfocused(&format!("{} 30 30", path_str(&agent)));
        harness::wait_until(|| hosted.agents().iter().any(|(p, _)| *p == pane));
    }
}
```

Add `pub fn wait_until(probe: impl FnMut() -> bool)` to `harness/mod.rs` (polls every 50 ms up to `DEADLINE`, panics with a message otherwise).

- [ ] **Step 3: Run to verify failure**

Run: `cargo nextest run -p amon-cli --test e2e hosted_runtimes`
Expected: compile error (no `harness::hosted`).

- [ ] **Step 4: Implement the harness**

`harness/mod.rs`: extract the body of `PtySession::start` into `pub fn start_program(sandbox: &Sandbox, program: &Path, argv: &[&str], env: &[(&str, &str)]) -> Self` and have `start` call it with `AMON`. In both `Sandbox::command` and `start_program` add `command.env("AMON_RUNTIMES_ROOT", sandbox.runtime_dir())` beside `AMON_RUNTIMES`; add `Sandbox::with_runtimes(self) -> Self` setting a flag that makes both emit `AMON_RUNTIMES=1`.

`harness/hosted.rs` (essentials; keep every function short):

```rust
pub fn binary(kind: Kind) -> PathBuf {
    // The sandbox PATH is closed on purpose; the runtime is found on the
    // *test process's* PATH — mise puts it there — and symlinked in.
    let path = std::env::var_os("PATH").expect("PATH");
    std::env::split_paths(&path).map(|dir| dir.join(kind.name())).find(|candidate| candidate.is_file())
        .unwrap_or_else(|| panic!("{} is not installed; run `mise install` (it is pinned in .mise.toml)", kind.name()))
}

impl Hosted {
    pub fn start(sandbox: &Sandbox, kind: Kind) -> Self {
        let bin = binary(kind);
        std::os::unix::fs::symlink(&bin, sandbox.runtime_path("bin").join(kind.name())).ok();
        let home = sandbox.runtime_path("home");
        let (config_env, socket) = match kind {
            Kind::Herdr => (("XDG_CONFIG_HOME", sandbox.runtime_path("home/.config")), home.join(".config/herdr/sessions/amon-e2e/herdr.sock")),
            Kind::Luvus => (("LUVUS_HOME", sandbox.runtime_path("home/.luvus")), home.join(".luvus/sessions/amon-e2e/luvus.sock")),
        };
        let env = [(config_env.0, config_env.1.to_str().unwrap()), ("SHELL", "/bin/sh"), ("TERM", "xterm-256color")];
        let client = PtySession::start_program(sandbox, &bin, &["--session", "amon-e2e"], &env);
        let hosted = Self { kind, session: "amon-e2e".into(), socket, client, … };
        super::wait_until(|| hosted.socket.exists() && hosted.try_request("ping", json!({})).is_ok());
        hosted
    }

    pub fn run_unfocused(&self, command: &str) -> String {
        let pane = match self.kind {
            Kind::Herdr => self.request("pane.split", json!({"direction": "right", "focus": false}))["pane_id"].as_str().unwrap().to_owned(),
            Kind::Luvus => self.request("pane.split", json!({"direction": "right", "focus": false}))["pane"].as_str().unwrap().to_owned(),
        };
        match self.kind {
            Kind::Herdr => self.request("pane.run", json!({"pane_id": pane, "command": command})),
            Kind::Luvus => self.request("pane.run", json!({"pane": pane, "command": command})),
        };
        pane
    }

    pub fn report(&self, pane: &str, status: &str) {
        match self.kind {
            Kind::Herdr => self.request("pane.report_agent", json!({"pane_id": pane, "source": "amon-e2e", "agent": "claude", "state": status})),
            Kind::Luvus => self.request("agent.report", json!({"pane": pane, "source": "amon-e2e", "agent": "claude", "status": status})),
        };
    }

    pub fn agents(&self) -> Vec<(String, String)> {
        let list = self.request("agent.list", json!({}));
        let (pane_key, status_key) = match self.kind { Kind::Herdr => ("pane_id", "agent_status"), Kind::Luvus => ("pane", "status") };
        list["agents"].as_array().map(|a| a.iter().filter_map(|x| Some((x[pane_key].as_str()?.to_owned(), x[status_key].as_str()?.to_owned()))).collect()).unwrap_or_default()
    }
}
```

The herdr method/param names above (`pane.split`/`pane.run`/`pane.report_agent` params, split result key) are the ones the herdr CLI usage strings imply; **confirm them on the first live run against `herdr api schema --json` and adjust** — the CLI forms in Global Constraints are the fallback if a method is CLI-only (`Command::new(bin).args(["--session","amon-e2e","pane","run",pane,command])` with the same env).

`detach` kills the PTY child (`self.client.kill()`); `reattach` starts a new `PtySession` the same way. `client_pid` comes from `PtySession::child.process_id()`.

- [ ] **Step 5: Run the smoke test**

Run: `cargo nextest run -p amon-cli --test e2e hosted_runtimes`
Expected: PASS for both kinds. If luvus never lists the agent, check `pane.processes` for the pane to see what luvus thinks is running (the fake agent must be the pane's foreground process with comm `claude`).

- [ ] **Step 6: Commit**

```bash
git add .mise.toml .github crates/amon-cli
git commit -m "The e2e harness runs real herdr and luvus sessions"
```

---

### Task 7: End-to-end: the daemon adopts real herdr and luvus sessions

**Files:**
- Modify: `crates/amon-cli/tests/e2e.rs` (`hosted_runtimes` module)

- [ ] **Step 1: Write the tests (one body, one `#[test]` per kind each)**

```rust
fn hosted_row(sandbox: &Sandbox, hosted: &Hosted, pane: &str) -> Option<serde_json::Value> {
    let socket = hosted.socket.to_string_lossy().into_owned();
    sandbox.status_json()["agents"].as_array()?.iter()
        .find(|a| a["runtime"]["socket"] == socket && a["runtime"]["pane"] == pane).cloned()
}

fn an_agent_in_a_pane_shows_up_with_its_runtime_and_no_window(kind: Kind) {
    let sandbox = Sandbox::new().with_runtimes();
    let agent = sandbox.fake_agent("claude", WORKING_THEN_IDLE);
    let hosted = Hosted::start(&sandbox, kind);
    let pane = hosted.run_unfocused(&format!("{} 30 30", path_str(&agent)));
    // The daemon comes up on the first `amon status`.
    let row = harness::wait_for(|| hosted_row(&sandbox, &hosted, &pane));
    assert_eq!(row["runtime"]["kind"], kind.name());
    assert_eq!(row["runtime"]["session"], "amon-e2e");
    assert!(row.get("window").is_none() || row["window"].is_null(), "no compositor here: {row}");
    assert!(row["id"].as_str().unwrap().starts_with(kind.name()));
}

fn a_status_report_reaches_the_row_through_the_event_path(kind: Kind) {
    let sandbox = Sandbox::new().with_runtimes();
    let agent = sandbox.fake_agent("claude", WORKING_THEN_IDLE);
    let hosted = Hosted::start(&sandbox, kind);
    let pane = hosted.run_unfocused(&format!("{} 30 30", path_str(&agent)));
    harness::wait_for(|| hosted_row(&sandbox, &hosted, &pane));
    hosted.report(&pane, "blocked");
    let started = std::time::Instant::now();
    harness::wait_for(|| hosted_row(&sandbox, &hosted, &pane).filter(|r| r["state"] == "blocked"));
    // The rescan is every 2 s; the event path is what makes this fast.
    assert!(started.elapsed() < std::time::Duration::from_millis(1500), "took {:?}", started.elapsed());
}

fn detaching_the_client_removes_the_rows_and_reattaching_restores_them(kind: Kind) {
    let sandbox = Sandbox::new().with_runtimes();
    let agent = sandbox.fake_agent("claude", WORKING_THEN_IDLE);
    let mut hosted = Hosted::start(&sandbox, kind);
    let pane = hosted.run_unfocused(&format!("{} 60 60", path_str(&agent)));
    harness::wait_for(|| hosted_row(&sandbox, &hosted, &pane));
    hosted.detach();
    harness::wait_until(|| hosted_row(&sandbox, &hosted, &pane).is_none());
    hosted.reattach(&sandbox);
    harness::wait_for(|| hosted_row(&sandbox, &hosted, &pane));
}

fn focusing_an_agent_lands_on_its_pane_and_marks_it_seen(kind: Kind) {
    let sandbox = Sandbox::new().with_runtimes();
    let agent = sandbox.fake_agent("claude", WORKING_THEN_IDLE);
    let hosted = Hosted::start(&sandbox, kind);
    let pane = hosted.run_unfocused(&format!("{} 60 60", path_str(&agent)));
    harness::wait_for(|| hosted_row(&sandbox, &hosted, &pane));
    // Finished while nobody was looking: the pane is not the focused one.
    hosted.report(&pane, "working");
    hosted.report(&pane, "idle");
    let row = harness::wait_for(|| hosted_row(&sandbox, &hosted, &pane).filter(|r| r["state"] == "idle" && r["seen"] == false));
    let id = row["id"].as_str().unwrap();
    let output = sandbox.run(&["focus", "--agent", id]);
    assert!(output.status.success(), "{output:?}");
    // The runtime itself now says the agent is idle-and-seen, and amon follows.
    harness::wait_until(|| hosted.agents().iter().any(|(p, s)| *p == pane && s == "idle"));
    harness::wait_for(|| hosted_row(&sandbox, &hosted, &pane).filter(|r| r["seen"] == true));
}
```

Plus the eight `#[test]` wrappers (`herdr_…`/`luvus_…` for each body). `harness::wait_for` is the `Option`-returning twin of `wait_until` (add it beside).

For herdr, `report("idle")` after `working` on an unfocused pane yields `done` (herdr derives done from idle+unseen); for luvus `report("done")` is accepted directly — make `report` in the harness map `"idle"` to what produces "finished, unseen" per runtime, or have the test call `report(&pane, "done")` and let the herdr adapter send `idle`. Choose the second (the adapter knows its runtime) and document it on `report`.

- [ ] **Step 2: Run**

Run: `cargo nextest run -p amon-cli --test e2e hosted_runtimes`
Expected: all eight pass. Failures here are the point of the exercise: each is either a harness assumption (fix in `hosted.rs`, confirmed against the runtime's schema) or a real defect in Tasks 3–5 (fix there, with a unit test).

- [ ] **Step 3: Run the whole suite and lint**

Run: `just test && just lint`
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add crates/amon-cli
git commit -m "End to end against real herdr and luvus sessions"
```

---

### Task 8: Docs

**Files:**
- Create: `docs/adr/0018-runtimes-live-behind-one-seam.md`
- Modify: `docs/research/herdr-live-integration.md` (append "## luvus" section), `README.md:33-36,106-113`, `CHANGELOG.md` (under `<!-- next -->`), `CONTEXT.md` if it lists the daemon's modules

- [ ] **Step 1: Write the ADR**

Title: *Runtimes live behind one seam*. Sections: the decision (a `Hosted` trait that is the list of what differed; generic discovery/supervision/projection), why not copy the herdr module, why the protocol carries a tagged `Runtime` rather than a per-runtime field or a map (consumers read `kind`; each variant owns its shape; unknown kinds fail loudly), why the wrapper steps aside on the runtime's own env var, and what the seam deliberately is not (no registry, no plugin loading; two impls and a match). Reference the spec and ADR-0016.

- [ ] **Step 2: Research note and README/CHANGELOG**

Append to the research note a "## luvus" section listing the deviations: global sequenced subscription with `after_sequence` and `events.resync_required`; stable pane ids so identity is the pane; no agent-appeared event so a label change is structural; `pane.focus` as the hop; `LUVUS_HOME` and the `/tmp/luvus-<uid>` alias; `AMON_RUNTIMES`/`AMON_RUNTIMES_ROOT`.

README: the herdr paragraph becomes "Agents you run in herdr or luvus count too…" with both links; the "Inside a herdr session" paragraph names both.

CHANGELOG under `<!-- next -->`:
```
- **Agents in luvus show up**: amon watches for
  [luvus](https://github.com/RizRiyz/luvus) sessions the way it does herdr's —
  their agents join the bar and the panel, `amon focus` lands on the pane, and
  `amon <agent>` inside a luvus pane runs the agent bare.
- **Protocol**: `AgentEntry.herdr` is now `AgentEntry.runtime`, tagged by
  `kind` (`herdr` | `luvus`). Nothing outside amon read the old field.
- The end-to-end suite now drives real herdr and luvus binaries, pinned in
  `.mise.toml`.
```

- [ ] **Step 3: Commit**

```bash
git add docs README.md CHANGELOG.md CONTEXT.md
git commit -m "Runtimes behind one seam, written down"
```

---

## Self-review

- Spec coverage: protocol enum + `focus_request` (T1); wrapper (T2); seam + herdr (T3); luvus (T4); focus hop rule + `AMON_RUNTIMES` (T3, T5); e2e + pins (T6, T7); docs (T8). `AMON_RUNTIMES_ROOT` is an addition beyond the spec, needed to keep the e2e hermetic on a developer's desktop; noted in the research note.
- Type consistency: `Hosted::event` takes `&HashMap<String, String>` everywhere; `HostedEvent::Status.agent` is `Option<Option<String>>` in T3 and T4; `Runtime::pane()/socket()/focus_request()` used in T3, T5, T7.
- The spec puts `PANE_ENV` on the trait; this plan keeps the list in `amon-protocol` instead because the wrapper does not depend on the daemon crate. Spec updated accordingly at execution time (one-line edit in "Wrapper").
