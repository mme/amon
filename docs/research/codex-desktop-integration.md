# Showing Codex desktop agents in amon, the way herdr agents are shown

Status: exploration (2026-08-30), against the Linux ChatGPT/Codex desktop
package 26.825.51511 (bundles codex 0.151.0-alpha.7.2), codex CLI 0.150.1
installed here, `openai/codex` at `28327355` (2026-08-30), and amon main
(`8a306a6`). Nothing here is implemented.

**Update, later on 2026-08-30 — verified live, see "Live verification"
below: the opt-in this note is built on does not work on any current desktop
build (26.820 and later, including the 26.825 read here). It is an upstream
regression, not a design problem; the last working build is 26.818.61809.
Work on this integration is parked until OpenAI ships a fix.**

The question: the herdr integration adopts agents amon never wrapped by
speaking herdr's socket and borrowing the herdr client's window. OpenAI's
desktop app is now on Linux and runs Codex threads that amon likewise never
wrapped. Can the same shape work — rows on the bar, `Super+N` landing on the
right thread — and where does it break?

Short answer: yes in principle, with one opt-in the user has to make — but
that opt-in is currently broken in the app (see "Live verification"). The
desktop app by default runs a **private** app-server over stdio that nobody
else can reach.
With `CODEX_APP_SERVER_USE_LOCAL_DAEMON=1` it instead attaches to the
**shared local app-server daemon** at
`~/.codex/app-server-control/app-server-control.sock`, which is a JSON-RPC
socket any local client can join, broadcasts thread status changes to every
connection, and is what OpenAI's own `codex agents` TUI browses. Jumping to a
thread is `codex://threads/<uuid>`, which the Linux package registers a URL
handler for. The rest is the herdr module with different nouns.

## How the herdr integration works today (the shape to copy)

From `docs/research/herdr-live-integration.md` and
`crates/amon-daemon/src/herdr/`:

1. **Discovery by `/proc`** (`herdr/proc.rs`): find attached herdr *clients*
   (a `herdr` process with a tty), derive the session socket. No client, no
   rows — an agent with no window to jump to is not shown.
2. **Window by ancestry** (`amon-hypr`): the client pid → nearest ancestor
   owning a Hyprland window; then follow `movewindow`/`closewindow`.
3. **One thread per session** (`herdr/supervisor.rs`): `ping`, subscribe,
   `session.snapshot`, project every agent as an `AgentEntry` with an id
   namespaced by socket digest + stable terminal id; status events patch
   the row in-stream; anything structural tears down and re-snapshots.
4. **State mapping** (`herdr/project.rs`): `idle`→Idle/seen,
   `done`→Idle/unseen, `working`→Working, `blocked`→Blocked, else Unknown.
5. **Provenance on the entry**: `AgentEntry::herdr: Option<HerdrInfo>`
   carries socket + pane so `amon focus` can do the second hop.
6. **Focus is two hops** (`amon-cli/src/focus.rs::go_to`): compositor
   focus on the window, then `agent.focus` over the socket. Every route
   goes through `focus.rs`; a test forbids widgets addressing windows
   themselves.
7. **The wrapper steps aside inside herdr** (`HERDR_ENV=1` → exec bare):
   one detection authority per context.

Everything below maps onto those seven points.

## What the Codex desktop app is, on Linux

Facts from the actual package (`chatgpt_amd64.deb`, downloaded from
`https://persistent.oaistatic.com/codex-app-prod/linux/deb/latest/`,
extracted under `~/.cache/claude-research/deb/`) and from launching the
extracted app once on this machine:

- Electron. `/usr/bin/chatgpt` → `/usr/lib/chatgpt/codex-launcher` (a
  two-line `exec …/ChatGPT "$@"` shell script) → `/usr/lib/chatgpt/ChatGPT`.
  Official docs: only `.deb`/`.rpm` for Ubuntu/Debian/Fedora; here it runs
  straight from the extracted tree (`--no-sandbox` needed without the
  setuid chrome-sandbox). No AUR package of the official binary was found
  by name (`chatgpt-desktop` exists on AUR; not inspected).
- `chatgpt.desktop` declares `Exec=chatgpt %U` and
  `MimeType=x-scheme-handler/codex;…` — so `xdg-open codex://…` reaches
  the app. Electron single-instance lock is taken; a second launch's argv
  is forwarded to the running instance (`second-instance` handler queues
  the deep link). Verified in `app.asar`.
- Hyprland sees one window: class/initialClass `Chatgpt`, title `ChatGPT`,
  **XWayland** (docs say native Wayland is experimental via
  `--ozone-platform=wayland`). The window's pid is the Electron main
  process.
- Codex runs as the app's bundled CLI: the main process spawns
  `/usr/lib/chatgpt/resources/codex -c features.code_mode_host=true
  app-server --analytics-default-enabled -c mcp_servers.codex_app=…` as a
  **direct child**, talking over pipes (stdio). Observed with `ss -xp`:
  only anonymous socketpairs between `ChatGPT` and `codex`; nothing under
  `~/.codex/app-server-control/` was created.
- The app talks to that app-server as `clientInfo {name: <r.K>, title:
  "Codex Desktop"}` and opts into `thread/status/changed`, `thread/started`,
  `turn/completed`, `serverRequest/resolved` notifications — the same
  events amon would want.
- It also opens `~/.codex/ipc/ipc.sock`, which is the IDE-context router
  (`tui/src/ide_context/ipc.rs`: private transport for `/ide`), not a
  control surface. Ignore it.
- It uses the user's `~/.codex` (`CODEX_HOME`) — same config, hooks,
  sessions, sqlite state as the CLI. (The bundled alpha migrated my state
  DB on that test run; see "Side effects" at the end.)

## The crux: stdio by default, shared daemon on opt-in

The relevant code in `app.asar` (deminified by hand; class `EH.connect`):

```
sock = join(CODEX_HOME, "app-server-control", "app-server-control.sock")
if platform != win32
   && no config overrides
   && hostConfig.kind == "local"
   && env.CODEX_APP_SERVER_USE_LOCAL_DAEMON == "1"
   && env.CODEX_APP_SERVER_FORCE_CLI != "1"
   && !env.CODEX_CLI_PATH
   && hostConfig.codex_cli_command == null
   && `<bundled codex> app-server daemon version` succeeds
      and reports appServerVersion >= "0.141.0"          // uv(): tv = "0.141.0"
then  kind = "websocket": connect ws://localhost/rpc over the unix socket
else  kind = "stdio":     spawn bundled codex app-server on pipes
```

And in `openai/codex` `codex-rs/app-server/src/lib.rs` the transport is
exclusive: `AppServerTransport::Stdio` starts a stdio connection and
**nothing else**; only `UnixSocket` calls `start_control_socket_acceptor`.
So in the default mode there is no socket for amon to join, full stop.

The daemon route is real and first-party, if undocumented for the desktop:

- `codex app-server daemon start|stop|restart|version|bootstrap` manages a
  pidfile-backed app-server on the control socket
  (`codex-rs/app-server-daemon/README.md`: "used by remote clients such as
  the desktop and mobile apps"). `start` launches the *standalone managed*
  binary `CODEX_HOME/packages/standalone/current/codex` (from
  `chatgpt.com/codex/install.sh`), not whatever is on PATH — but the socket
  is just `codex app-server --listen unix://`, and the desktop's probe only
  asks the socket its version, so a manually started
  `codex app-server --listen unix://` from the mise-installed CLI should
  satisfy it too (untested).
- OpenAI's own multi-client surface: `codex agents` — "Browse all agent
  sessions on the shared local app-server daemon". Issue #31991 documents a
  user running Desktop + mobile SSH proxy on the same socket
  (macOS, `launchctl setenv CODEX_APP_SERVER_USE_LOCAL_DAEMON 1`).
- The CLI auto-attaches too: `tui/src/lib.rs::maybe_probe_default_daemon_socket`
  (unix only) — if the control socket answers and the invocation has no
  `-c` overrides, a plain `codex` becomes a thin client of the daemon
  (`AppServerTarget::LocalDaemon`) instead of embedding its own core.

On Hyprland/Omarchy the opt-in is an environment variable visible to the
app's launcher — `~/.config/environment.d/` or uwsm env — plus a running
daemon. That is a setup step amon could own (`amon setup` → desktop
integration for codex), the way it owns hooks, aliases and shims today.

## What the shared daemon exposes

Protocol: JSON-RPC 2.0 without the `jsonrpc` header, one message per
websocket text frame; the unix socket carries a standard HTTP Upgrade
handshake first (`codex-rs/app-server/README.md` "Protocol"). Schema dumps:
`codex app-server generate-json-schema`. Per connection: `initialize`
(with `clientInfo`, optional `capabilities.experimentalApi`,
`optOutNotificationMethods`) then the `initialized` notification.

What amon needs, all present:

| need | method / notification | notes |
| --- | --- | --- |
| bootstrap | `thread/loaded/list` → ids in memory; `thread/list` (filters: `cwd`, `sourceKinds`, `archived`, `sortKey`) → `Thread {id, cwd, source, name, preview, status, git_info, created_at, …}` | `thread/list` defaults to `sourceKinds` "interactive only" (`cli`, `vscode`) |
| live state | `thread/status/changed {threadId, status}` | **broadcast to every connection** — `thread_status.rs::mutate_and_publish` → `OutgoingMessageSender::send_server_notification` → `send_server_notification_to_connections(&[])` → `OutgoingEnvelope::Broadcast` |
| arrivals / departures | `thread/started` (broadcast, `thread_processor.rs:1589`), `thread/closed`, `thread/archived` | a thread with no subscribers and no activity is unloaded after 30 min → `status: notLoaded` then `thread/closed` |
| status vocabulary | `ThreadStatus = notLoaded \| idle \| systemError \| active {activeFlags: [waitingOnApproval \| waitingOnUserInput]}` (`app-server-protocol/src/protocol/v2/thread.rs:1634`) | exactly amon's Working/Blocked split |
| jump | not in the protocol; `codex://threads/<uuid>` deep link (app.asar route parser: host `threads`, one UUID path segment → `localConversation`) | archived threads open blank (issue #18216) |

Not needed but there: `thread/read`, `thread/turns/list`, `turn/start`,
`turn/interrupt`, approvals as server→client requests, `server/diagnostics`.
Subscribing to a thread's turn/item stream (`thread/resume`) is **not**
required for status — status is global.

Everything read-only here is side-effect free; there is no "mark seen"
call, unlike herdr's `agent.focus`.

## State mapping

| app-server | amon | seen |
| --- | --- | --- |
| `active`, flags `[]` | Working | — |
| `active`, `waitingOnApproval` or `waitingOnUserInput` | Blocked | false |
| `idle` | Idle | see below |
| `systemError` | Unknown (ranks last; a crashed thread must not cry wolf on every bar) — revisit once seen in the wild | — |
| `notLoaded` / `thread/closed` | retire the row | — |

**Seen has no source.** herdr told us `done` vs `idle`; the app-server does
not know whether anyone looked, and the desktop app is not a terminal so
there is no focus reporting (ADR-0007). The honest options: (a) treat
`active → idle` as Done until the `Chatgpt` window next receives compositor
focus (Hyprland `activewindow` events, which `amon-hypr::follow` does not
parse yet), or (b) never claim Done for desktop rows. (a) is a coarser
proxy than a terminal view but it is the same signal the wrapper uses at
launch ("focused until told otherwise"); the desktop app is a single window
so window focus ≈ view focus more often than not. Decide when implementing.

## Identity, and not showing one thread twice

- Row id: `codex-desktop:<socket digest>:<threadId>` — thread ids are
  UUIDs and stable across restarts; the socket digest keeps `CODEX_HOME`s
  apart exactly as it keeps herdr sessions apart.
- `cwd` and `git_info` come from `thread/list`; project/branch can be
  derived from cwd the way herdr rows (deliberately) do not yet.
- **Wrapped CLI overlap.** Once a daemon socket exists, `amon codex` spawns
  a TUI that silently attaches to that daemon, so the same thread would
  be a wrapped row (screen-detected) *and* a daemon thread. And the
  wrapper's hook-based `agent_session_id` will be **absent** for those:
  hooks run inside core, i.e. inside the daemon, where `AMON_ENV` /
  `AMON_SOCKET_PATH` are not set, so `amon-agent-state.sh` exits at its
  first guard. The clean rule: the daemon module projects only threads
  whose `source != cli`. CLI threads are the wrapper's job, and a bare
  `codex` the user never aliased is invisible today already. What source
  the desktop passes on `thread/start` is **unverified** (the app's
  handoff rollout writer stamps `source: "vscode", originator: "Codex
  Desktop"`; the live value needs one daemon session to confirm — it may
  also be a `Custom("…")`).
- The wrapper does not need a `HERDR_ENV`-style bypass: the desktop never
  launches through a shell or shim.

## Finding the window, and the focus flow

- Discovery: unlike herdr there is no "client process with a tty". Scan
  `/proc` for the Electron main process (`/usr/lib/chatgpt/ChatGPT`, or
  `chatgpt` via the launcher; the main process is the one whose parent is
  not itself `ChatGPT`), or ask Hyprland for `class == "Chatgpt"`. The
  first is consistent with the herdr module (pid → `amon-hypr::resolve`);
  the second is simpler but names the compositor's class, which
  `amon-hypr` was built to avoid. Either way the window follows
  `movewindow`/`closewindow` via the existing `WindowWatch`.
- Gate: as with herdr, *no app window → no rows* — a daemon serving only
  `codex agents` in a terminal has no place to jump to. (A daemon-hosted
  CLI thread does have a window: the wrapper's. Excluded by the source
  rule above, not by this gate.)
- Jump: `amon focus --agent <id>` → `go_to`: compositor-focus the
  `Chatgpt` window (existing hop), then the second hop is
  `xdg-open codex://threads/<threadId>` (or `chatgpt codex://…` directly —
  `Exec=chatgpt %U` is what xdg would run). The running instance receives
  it via single-instance forwarding and navigates. Add `AgentEntry::codex:
  Option<CodexDesktopInfo { socket, thread_id }>` beside `herdr`, additive
  and skip-serialized like `HerdrInfo`, so `focus.rs` stays the one place
  that knows the hop.
- Open question: whether a deep link to a thread already open in the
  window's *other* tab/project switches reliably; issue #23900 reports
  hidden-thread misses on Windows. Test on Linux before relying on it.

## What the daemon's codex module amounts to

Mirror of `herdr/supervisor.rs`, with the herdr-specific parts swapped:

1. Watch for the control socket (`inotify` on
   `~/.codex/app-server-control/`, or the 2 s rescan the herdr watcher
   already runs) and for the `Chatgpt` main process; resolve its window.
2. Connect: HTTP Upgrade over the unix socket, `initialize` as
   `clientInfo {name: "amon", title: "amon", version}` with
   `optOutNotificationMethods` for everything `item/*` and `turn/*` except
   `turn/completed`; `initialized`; `thread/loaded/list` + `thread/list`
   (page until done, `sourceKinds` unset, `archived: false`); project the
   loaded, non-`cli` ones.
3. Patch rows from `thread/status/changed`; re-list on `thread/started`,
   `thread/closed`, `thread/archived`; reconnect + re-list on socket drop
   (the daemon's updater restarts app-server under a running desktop, so a
   drop is not "codex exited").
4. Timestamp transitions on receipt, as for herdr (`recencyAt` exists but
   only advances at turn start).
5. Setup: `amon setup` gains a "codex desktop" integration that (a)
   ensures a daemon is running — `codex app-server daemon start` where the
   standalone install exists, else document `codex app-server --listen
   unix://` — and (b) puts `CODEX_APP_SERVER_USE_LOCAL_DAEMON=1` where the
   desktop launcher sees it. `amon doctor` checks both plus the
   `>= 0.141.0` version gate. Without this the module simply finds no
   socket and shows nothing — never guesses from the private stdio mode.

## Live verification (2026-08-30, later)

The section above was written from reading the app's code. It was then put
to the test on this machine, against the AUR-installed package 26.820.80927
(bundled codex 0.150.0-alpha.8) and a daemon started from the mise-installed
CLI 0.150.1 with `codex app-server --listen unix://`.

**The daemon side works exactly as described.** The socket appears at
`~/.codex/app-server-control/app-server-control.sock`; both the CLI's and the
bundled binary's `app-server daemon version` report `appServerVersion
0.150.1` (above the app's `0.141.0` gate); a raw websocket client — HTTP
Upgrade on the unix socket, masked text frames — completes `initialize`
(`userAgent`, `codexHome`, `platformOs`) and `thread/loaded/list`, and
receives broadcast notifications (`remoteControl/status/changed`). The
scratch client is ~60 lines of Python; no library is needed.

**The desktop never attaches.** Three launches, all logging
`[AppServerConnection] Starting app-server connection hostId=local
transport=stdio`:

1. Through omarchy's launcher (`uwsm-app -- chatgpt`) after
   `systemctl --user set-environment CODEX_APP_SERVER_USE_LOCAL_DAEMON=1`
   **and** `systemctl --user restart wayland-wm-app-daemon.service` — the
   launcher is a long-lived user service that captured its environment at
   login, so the restart is what makes the variable reach apps it starts.
   (Chromium overwrites its own `/proc/<pid>/environ`, so the app's
   environment cannot be read back; the socket connection count and the log
   are the evidence.)
2. Launched directly from a shell that had the variable.
3. With the variable additionally exported in the login shell (the app's
   "shell environment hydrated" startup phase was a suspect; it is not the
   cause).

The cause is in the app. The attach condition in `app.asar` (class `QB`,
this build) is the one quoted above plus `!e?.length`, where
`e = await this.options.getConfigOverrides()` on a local host. That function
is the injector for the bundled **codex-app-tools MCP plugin**: it creates
the app-tools pipe, sets `CODEX_APP_TOOLS_PIPE_PATH`, reads the bundled
`codex-app-tools/desktop-mcp.json`, and returns a request-level override
`mcp_servers.codex_app={…}`. Its failure paths (`missing-pipe`,
`missing-plugin`, `missing-definition`) return a one-element
`mcp_servers.codex_app={command="",enabled=false}` override. So on a local
host the list is never empty, `!e?.length` is always false, and the
websocket branch is unreachable: the app hard-wires itself to a private
child so its app-tools plugin can be configured on it. Nothing in the user's
environment changes that.

This is a known upstream regression, filed the same week:

- openai/codex#41014 — "[macOS][regression] `codex_app` MCP override makes
  `CODEX_APP_SERVER_USE_LOCAL_DAEMON=1` unreachable" (2026-08-27; same
  diagnosis, from the packaged startup logic).
- openai/codex#41112 — "[macOS] Desktop 26.820.60940 ignores local App
  Server daemon" — last working 26.818.61809, broken from 26.820.60940;
  logs went from `transport=websocket` to `transport=stdio`.
- openai/codex#40134 — the feature request for a documented
  `--remote unix://…` / setting instead of the env var.

None had a maintainer response as of this writing. The 26.825.51511 package
this note was first written against has the same code, so the "yes, with one
opt-in" verdict above was never true for a Linux build.

**The last working build attaches, on Linux too.** OpenAI's apt pool still
serves `chatgpt_26.818.61809_amd64.deb` (bundled codex 0.149.0-alpha.4.3).
Its `app.asar` has no `getConfigOverrides` and no `mcp_servers.codex_app`
at all; the condition is just platform, host kind, the env var, the
`CODEX_CLI_PATH`/`FORCE_CLI` escapes, and the version probe. Run from the
extracted tree (`--no-sandbox`, an isolated `CODEX_HOME` and
`--user-data-dir`, alongside the installed app) against a daemon in that
`CODEX_HOME`, it logged `Transport start success connectionId=1
hostId=local transport=websocket` and held one connection on the socket.

### The recipe, as it is meant to work

For when a fixed desktop ships (or on ≤ 26.818):

1. **Server.** Install the standalone package —
   `curl -fsSL https://chatgpt.com/codex/install.sh | sh` (Linux is
   supported; it lands under `~/.codex/packages/standalone/current/` and
   includes `codex-code-mode-host`, which the desktop's code-mode features
   need and which the Homebrew/mise/npm CLI does not ship, per #31991) —
   then `codex app-server daemon start`, or `daemon bootstrap` for a
   self-updating one. Any CLI ≥ 0.141 running
   `codex app-server --listen unix://` passes the desktop's probe, but
   without code-mode-host some desktop features would be missing.
2. **Client.** `CODEX_APP_SERVER_USE_LOCAL_DAEMON=1` in the desktop's own
   process environment, then a full restart. On omarchy/Hyprland the app is
   launched through `uwsm-app` → `wayland-wm-app-daemon.service`, so the
   variable belongs in the `systemd --user` environment:
   `~/.config/environment.d/50-codex.conf` for persistence, and for the
   running session `systemctl --user set-environment …` followed by
   `systemctl --user restart wayland-wm-app-daemon.service`. Login-shell
   exports are irrelevant.
3. **Check.** `ss -xp | grep app-server-control` shows the desktop's
   connection; the app's stderr says `transport=websocket`.

### Decision

Parked. amon will not pin users to 26.818, and a hook-based channel
(`~/.codex/hooks.json` runs inside the private app-server too: `SessionStart`,
`SessionEnd`, `UserPromptSubmit`, `PreToolUse`, `PostToolUse`,
`PermissionRequest`, `Stop`, `SubagentStop`, with `session_id`, `cwd` and a
`client` field) would work on every version but is a different, larger
design with no better answer to "seen". The socket design above stands; it
resumes when a desktop release attaches again — watch #41014 / #41112.

## What does not work, and was considered

- **Default (stdio) mode is opaque.** The only observable facts are
  process-level (a `codex … app-server` child of `ChatGPT` exists). Thread
  state could be scraped from `~/.codex/state_5.sqlite` (`threads` table:
  id, cwd, updated_at, rollout_path) and the append-only rollout JSONL
  under `~/.codex/sessions/`, but that is polling private files whose
  schema the bundled alpha just migrated under me; the app-server README
  even warns `thread/list` "scan-and-repair"s them. Not worth building.
- **No screen to detect.** The desktop is not a terminal, so the vendored
  codex manifest (`osc_title_working`, "Action Required") never applies.
- **Hooks as the channel.** `SessionStart`/`SessionEnd` hooks do run for
  daemon threads (`SessionEnd` "runs before archive, delete, and graceful
  app-server shutdown"), but there are no per-turn state hooks that would
  give Working/Blocked, and the amon hook script needs the wrapper's env.
  Could report thread ids to the daemon for correlation later; not needed
  for the first cut.
- **Remote control** (`codex remote-control`, the hosted relay the app
  uses for mobile) is orthogonal — it is how *other machines* reach this
  daemon; amon is local.

## Side effects of this research on this machine

Kept for honesty; nothing in the repo changed besides this file.

- The package was downloaded and extracted under `~/.cache/claude-research/`
  and the extracted app launched once for ~40 s (`--no-sandbox`). Both the
  extraction and the `~/.config/Codex/` user-data directory it created were
  deleted afterwards.
- The live verification later that day: a daemon from the mise CLI was
  started and stopped; `CODEX_APP_SERVER_USE_LOCAL_DAEMON=1` was set in the
  `systemd --user` environment and unset again, `wayland-wm-app-daemon`
  restarted twice, the installed desktop app quit and relaunched several
  times (once directly from a shell), and a marked export line was added to
  and removed from `~/.bashrc`. `chatgpt_26.818.61809_amd64.deb` was
  downloaded and extracted under `~/.cache/claude-research/deb-26.818/`,
  and run once from there with its own `CODEX_HOME`
  (`~/.cache/claude-research/codex-home`) and user-data dir, so the
  installed app's state was not touched by the older binary. An empty
  `~/.codex/app-server-control/` directory remains.
- What could not be undone: the bundled codex 0.151.0-alpha.7.2 touched
  `~/.codex/` (`sqlite/codex-dev.db`, `ipc/`, `vendor_imports/`,
  `plugins/cache/openai-bundled`, `.tmp/`, WAL files of `state_5.sqlite`
  and `logs_2.sqlite`). The installed CLI 0.150.1 still worked afterwards
  (`codex --version`); if `codex` misbehaves, this is the first suspect.

## Sources

Primary, in order of weight:

- The Linux package itself: `https://persistent.oaistatic.com/codex-app-prod/linux/deb/latest/chatgpt_amd64.deb`
  (26.825.51511, since deleted) — `control`, `postinst` (apt repo
  `…/codex-app-prod/linux/deb`, suite `stable`), `usr/share/applications/chatgpt.desktop`,
  `usr/lib/chatgpt/codex-launcher`, `resources/codex --version`,
  `resources/app.asar` strings (`CODEX_APP_SERVER_USE_LOCAL_DAEMON`,
  `app-server-control.sock`, `tv = "0.141.0"`, `clientInfo … "Codex Desktop"`,
  deep-link router, `second-instance`).
- Live run of that package on this Hyprland session: `hyprctl -j clients`,
  `/proc/<pid>/cmdline`, `ss -xp`.
- `openai/codex` @ `28327355`:
  `codex-rs/app-server/README.md` (protocol, transports, `thread/list`,
  `thread/loaded/list`, `thread/status/changed`, `thread/unsubscribe`
  unload timing, initialization);
  `codex-rs/app-server/src/lib.rs` (transport exclusivity);
  `codex-rs/app-server/src/thread_status.rs` and
  `src/outgoing_message.rs` (broadcast);
  `codex-rs/app-server/src/request_processors/thread_processor.rs` (`thread/started`);
  `codex-rs/app-server-protocol/src/protocol/v2/thread.rs` (`ThreadStatus`,
  `ThreadActiveFlag`) and `thread_data.rs` (`Thread`);
  `codex-rs/app-server-transport/src/transport/mod.rs` (socket path);
  `codex-rs/app-server-daemon/README.md` and `src/lib.rs`;
  `codex-rs/tui/src/lib.rs` (`maybe_probe_default_daemon_socket`,
  `app_server_target_for_launch`, `can_reuse_implicit_local_daemon`);
  `codex-rs/tui/src/ide_context/ipc.rs`;
  `codex-rs/protocol/src/protocol.rs` (`SessionSource`);
  `codex-rs/cli/src/desktop_app/{mod,mac,windows}.rs` (no Linux
  `codex app` launcher; `codex://threads/new` builder);
  `codex-rs/hooks/src/types.rs`.
- Local CLI 0.150.1: `codex --help`, `codex app-server --help`,
  `codex app-server daemon --help`, `codex agents --help`,
  `codex app-server proxy --help`.
- OpenAI docs: https://learn.chatgpt.com/docs/linux/linux-app (packages,
  distros, `chatgpt --ozone-platform=wayland`, XWayland default);
  https://learn.chatgpt.com/docs/app.
- amon: `docs/research/herdr-live-integration.md`,
  `crates/amon-daemon/src/herdr/{proc,supervisor,project}.rs`,
  `crates/amon-cli/src/focus.rs`, `crates/amon-protocol/src/agent.rs`,
  `crates/amon-wrapper/src/lib.rs` (`HERDR_ENV`), `~/.codex/hooks.json`
  and `~/.codex/amon-agent-state.sh` (installed codex integration).

- Live verification (2026-08-30): the installed AUR package
  `openai-codex-desktop 26.820.80927` (`/usr/lib/chatgpt/resources/app.asar`,
  class `QB`, `getConfigOverrides` → the app-tools injector and its `GT`
  fallback); `chatgpt_26.818.61809_amd64.deb` from
  `https://persistent.oaistatic.com/codex-app-prod/linux/deb/pool/main/c/chatgpt/`
  (the `Packages` index there lists 26.825.51511 as current);
  `https://chatgpt.com/codex/install.sh` (read, not run);
  `codex-rs/app-server-daemon/README.md` and `codex-rs/hooks/src/{lib,types}.rs`
  @ `88f7765` (2026-08-30).
- https://github.com/openai/codex/issues/41014, #41112, #40134 — the
  regression and the feature request, see "Live verification".

Secondary (reports, not verified against code beyond what is above):

- https://github.com/openai/codex/issues/31991 — Desktop on the managed
  local daemon via `CODEX_APP_SERVER_USE_LOCAL_DAEMON=1` (macOS).
- https://github.com/openai/codex/issues/21779, #28677, #23900, #18216 —
  deep-link requests and failure modes.
- https://gist.github.com/zhuowei/98005fb9f2a42d5fd376f0fa71f204cc —
  `codex://` scheme list from the macOS app's asar.
- https://github.com/openai/codex/issues/21551 — multi-client co-presence
  RFC (turn *item* streams to a second subscriber were unreliable; status
  broadcast is a separate, simpler path).
- https://github.com/kcosr/codex-threads, https://github.com/0xcaff/codex-web
  — third-party clients of a shared `codex app-server --listen unix://`.
- Press on the Linux release (2026-08-11):
  https://www.phoronix.com/news/ChatGPT-Desktop-Linux-Preview,
  https://thenewstack.io/openais-chatgpt-desktop-linux/,
  https://www.techrepublic.com/article/news-openai-chatgpt-codex-linux-desktop-preview/.
