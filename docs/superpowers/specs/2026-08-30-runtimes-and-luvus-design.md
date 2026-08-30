# Agent runtimes behind one seam, and luvus as the second one

Status: approved design, 2026-08-30. Against amon v0.2.0 (`8a306a6`), herdr
0.8.2 (protocol 21) and luvus 0.13.2 (UHP 1.0, `ping` protocol 1).

## Why

amon v0.2.0 shows agents living inside herdr sessions: a daemon module finds
attached herdr clients in the process table, speaks herdr's socket, projects
its agents into the registry, and `amon focus` does a second hop to the pane.
[luvus](https://github.com/RizRiyz/luvus) is a second runtime of the same
shape — detached server, tty-owning client, JSON-lines control socket, the
same five agent states — and users of it deserve the same treatment. Doing it
by copying the herdr module would leave two implementations of "watch a
runtime's clients and project its agents", which is exactly the thing that
drifts. So the herdr module becomes the first implementation behind a
runtime seam, luvus the second, and the protocol stops naming herdr.

## Protocol

`AgentEntry.herdr: Option<HerdrInfo>` is replaced by
`AgentEntry.runtime: Option<Runtime>`. No compatibility alias: the field
shipped in v0.2.0 hours ago and has no known readers outside this repository.
`PROTOCOL_VERSION` stays 1; `docs/protocol.schema.json` is regenerated.

```rust
/// Where a runtime-hosted agent lives. Present exactly on entries a runtime
/// module projects; wrapped agents never carry it. Each variant carries what
/// the focus hop needs and nothing else.
#[derive(Serialize, Deserialize, JsonSchema, ...)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Runtime {
    Herdr { socket: String, session: Option<String>, pane: String },
    Luvus { socket: String, session: Option<String>, pane: String },
}
```

On the wire: `"runtime": {"kind": "luvus", "socket": "…", "pane": "7"}`.
`kind` is the discriminator a badge or a filter reads; each variant owns its
own field set, so a future runtime that addresses agents by a token and a
URL adds a variant with those fields rather than bending these. Fields that
vary *within* a kind become an `Option` on that variant; there is no
free-form map.

`Runtime` gets one method, `focus_request(&self) -> (method, params)`,
returning the runtime's own focus call (`agent.focus {target}` for herdr,
`pane.focus {pane}` for luvus) — so the CLI hop and the e2e tests share one
definition of "the second hop" and neither re-derives it.

Entry ids stay `herdr:<socket digest>:<terminal_id>`; luvus entries are
`luvus:<socket digest>:<pane>`.

## Daemon: the seam

`crates/amon-daemon/src/herdr/` becomes `crates/amon-daemon/src/runtime/`:

```
runtime/
  mod.rs          the trait, the common types, spawn()
  discover.rs     the /proc scanner (was herdr/proc.rs::sessions), generic
  supervisor.rs   watch loop, handover, window follow, reconcile — generic
  project.rs      state mapping and entry construction — generic
  herdr/          classify, socket_for, wire: herdr's answers
  luvus/          classify, socket_for, wire: luvus's answers
```

The trait is the list of things that turned out to differ:

```rust
pub trait Hosted: 'static {
    const KIND: RuntimeKind;
    /// Process comm of both the client and the server.
    const COMM: &'static str;

    fn classify(argv: &[String], is_session_leader: bool, has_tty: bool, environ: &str) -> Role;
    fn socket_for(argv: &[String], environ: &str) -> Option<PathBuf>;

    /// Connect, gate on the protocol, subscribe, and snapshot. Returns the
    /// agents now and a stream of what changes.
    fn attach(socket: &Path, known: &[HostedAgent]) -> io::Result<(Vec<HostedAgent>, Box<dyn Events>)>;
    fn runtime(session: &Session, agent: &HostedAgent) -> Runtime;
}

pub struct HostedAgent {
    /// Stable within one life of the agent: herdr's terminal_id, luvus's pane id.
    pub identity: String,
    pub pane: String,
    pub agent: Option<String>,
    pub status: String,
    pub cwd: Option<String>,
    pub name: Option<String>,
    /// herdr's state_change_seq; None where the runtime has no such counter.
    pub life: Option<u64>,
}

pub enum HostedEvent {
    /// A status change on an agent already known.
    Status { identity: String, status: String },
    /// Anything else: resnapshot.
    Structural,
}
```

The supervisor keeps its current structure — one thread per attached
session, structural events tear down and resnapshot, status events patch in
place — and becomes generic over `H: Hosted`. `spawn()` starts one watcher
per runtime; `discover.rs` scans `/proc` once per tick for both comms.

### herdr under the seam

Behaviour is unchanged: per-pane `pane.agent_status_changed` subscriptions
plus the global structural set, `session.snapshot` nested or flat, `done` →
`Idle` + unseen, `state_change_seq` going backwards → new life. The
`attach` implementation owns the resubscribe-when-the-pane-set-changed
dance that today lives in the supervisor.

### luvus under the seam

- **Processes.** comm `luvus`. Clients: bare `luvus`, `luvus --session X`,
  `luvus --session=X`, `luvus session attach X`, `luvus attach <id>`,
  `luvus client`, `luvus --local`; all have a tty. Server: argv contains
  `server`; session name from `--session` in argv, else `LUVUS_SESSION` in
  environ, else default. `luvus --remote H` and `remote-client-bridge` are
  not local clients. Print-and-exit forms (`--version`, `doctor`, `uhp
  schema`, any other CLI subcommand) are `Other`.
- **Socket.** `$LUVUS_HOME` (default `~/.luvus`, `~/.luvus-dev` for debug
  builds — amon only knows the release name) then `luvus.sock` for the
  default session or `sessions/<name>/luvus.sock`. luvus aliases paths of
  100+ bytes into `/tmp/luvus-<uid>/<fnv64 hex>-api.sock`; amon reproduces
  that hash so long homes still resolve. `luvus-client.sock` is the binary
  TUI transport and is never touched.
- **Wire.** Same `{"id","method","params"}` lines and `{"id","result"}` /
  `{"id","error"}` replies as herdr. `ping` must answer `protocol: 1`.
  Bootstrap is `agent.list` (`agents[]` with `pane, agent, name, status,
  cwd`); `session.snapshot` is not needed. One `events.subscribe
  {after_sequence}` per session — global and sequenced; the last seen
  `sequence` is passed on reconnect so a blip loses nothing, and
  `events.resync_required` is structural.
- **Event mapping.** `pane.agent_status_changed {pane, status, agent}` for
  an owned pane whose `agent` still matches is `Status`; the same event for
  an unknown pane, or with a different or empty `agent` (the agent exited
  and the pane is a shell again), is `Structural`. `pane.closed`,
  `pane.created`, `pane.moved` are structural. Everything else is ignored.
- **Identity.** luvus pane ids are a monotonic `u32` for the pane's life and
  do not change on move, so `identity == pane`. There is no
  `state_change_seq`; an agent restarting in the same pane between two
  snapshots shows up as the `agent` field flipping through empty and is
  caught by the rule above.
- **Focus.** `pane.focus {pane}` switches workspace and tab and marks the
  pane seen (`done` → `idle`), the same semantics as herdr's `agent.focus`.

## Wrapper

`amon <agent>` steps aside — `exec_bare`, environment scrubbed — when either
`HERDR_ENV=1` or `LUVUS_ENV=1` is set. One detection authority per context,
as ADR-0016 already states for herdr. The list of pane env vars lives in
`amon-protocol` beside the other environment contracts, because the wrapper
does not depend on the daemon crate.

## `amon focus`

The hop runs for any entry with a `runtime`. Today the CLI hops only after
a successful window dispatch; an entry the compositor never placed (over
ssh, no Hyprland, the headless e2e) then gets no hop at all, though the hop
is the only part of the jump amon *can* do for it. New rule: dispatch the
window if there is one, then hop if there is a runtime; the return value
reports whether anything was done. The hop uses `Runtime::focus_request`.

`AMON_HERDR=0` becomes `AMON_RUNTIMES=0` (disables every runtime module),
and `AMON_RUNTIMES_ROOT=<dir>` limits adoption to sessions whose socket lies
under that directory — the e2e sandbox sets both so a developer's live
session is never adopted by a test daemon.

## End-to-end tests against the real runtimes

`.mise.toml` pins `ubi:ogulcancelik/herdr = "0.8.2"` and
`ubi:RizRiyz/luvus = "0.13.2"`, so CI (`mise install`) and developers run the
same binaries, and bumping one is a reviewed diff whose CI run is the
compatibility check. The e2e tests fail, not skip, when a binary is missing:
a silently skipped integration test is no test.

`crates/amon-cli/tests/harness` grows a `Hosted` sandbox helper that, inside
the existing `Sandbox` (`HOME`, `PATH`, sockets all under a temp dir,
`HYPRLAND_INSTANCE_SIGNATURE` removed):

1. starts the runtime's client on a harness-owned PTY with `--session
   amon-e2e` and `HERDR_CONFIG`/`LUVUS_HOME` inside the sandbox, which
   brings up the detached server;
2. waits for the API socket, then runs the fake `claude` in a pane through
   the runtime's own CLI (`herdr pane run` / `luvus pane run`);
3. exposes `report(status)` (`herdr pane report-agent` / `luvus agent report
   --source amon-e2e --kind claude --status …`), `focused_pane()` (asked
   over the socket), and `detach()` (kill the client pid).

Each test runs once per runtime (a `#[test]` per runtime calling a shared
body — nextest lists them separately):

- an agent in a pane appears in `amon status` with `runtime.kind` set, no
  window, and the daemon's state matching the runtime's;
- a status report reaches the row in well under the 2 s rescan (the event
  path works);
- detaching the client removes the row, reattaching restores it;
- `amon focus --agent <id>` lands the runtime's focus on the agent's pane
  (verified through the runtime's API), and a `done` agent is idle+seen
  afterwards;
- inside a pane, `amon claude` execs bare (`AMON_ENV` unset), extending the
  herdr-only test that exists.

CI's `socat` install step gains nothing; `mise install` already covers the
binaries. The job runs on `ubuntu-latest` as today.

## Out of scope

- Hyprland/omarchy in CI (Tier 2): a later spike, separate workflow.
- Reporting into either runtime (`pane.report_agent`, notifications).
- luvus's richer per-agent data (tokens, cost, `authority`).
- A runtime registry or plugin mechanism: two implementations and a match.

## Docs

- ADR: "Runtimes live behind one seam" — the trait is the list of what
  differed, the protocol names the kind, the wrapper steps aside on the
  runtime's own env var.
- `docs/research/herdr-live-integration.md` gains a section on luvus's
  deviations (global subscription, stable pane ids, `pane.focus`).
- README and CHANGELOG mention luvus beside herdr.
