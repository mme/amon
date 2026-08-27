# Showing herdr-hosted agents in amon, without wrapping herdr

Status: exploration (2026-08-27), against herdr v0.8.2 source and amon main
(`fde70bb`). Nothing here is implemented yet.

herdr became a client/server runtime: a background server owns the agent PTYs,
TUI clients attach from any terminal, and everything is controllable over a
local socket. Agents running in herdr panes are invisible to amon today — they
are not wrapped, and their processes do not live under any window. This doc
records how amon can adopt them anyway, and the decisions taken.

## What herdr exposes

One JSON-lines API socket per session, no auth beyond 0600 perms:

```
~/.config/herdr/herdr.sock                  # default session
~/.config/herdr/sessions/<name>/herdr.sock  # named sessions
```

- `ping` → `{version, protocol}` (protocol 21 at 0.8.2; additive growth is the
  norm, hard bumps are rare and per-release). A full JSON Schema of the API
  ships in the binary: `herdr api schema --json`.
- `session.snapshot` → workspaces, tabs, panes, agents in one bootstrap.
- `events.subscribe` → push stream (100 ms tick, 512-event ring):
  `pane.created/closed/moved/exited`, `pane.agent_detected`,
  `workspace.*`, `tab.*` are global; `pane.agent_status_changed` is
  **per-pane only** — one subscription per agent pane, re-subscribed as panes
  appear.
- `agent.list` / `agent.get` → per-agent `agent_status`
  (`idle|working|blocked|done|unknown`), agent kind, titles, cwd, pane id,
  `state_change_seq` (monotonic counter, no clock — amon timestamps
  transitions itself).
- `agent.focus <target>` → switches the attached client to that agent's
  workspace/tab/pane. Marks it seen (`done` → `idle`), which is exactly what
  a jump-to-agent wants.
- Speak the socket directly, never shell out to the `herdr` CLI on hot paths:
  the CLI hard-fails on any protocol version inequality; the raw socket
  serves any client.

State semantics are aligned by construction: amon vendors herdr's detect
engine and manifests byte-for-byte (ADR-0001), and herdr's `done` is amon's
finished-but-unseen — herdr tracks seen at pane granularity, finer than amon
can know for a plain terminal window.

## Finding the window (no wrapping needed)

The `herdr` process a user types into a terminal *becomes* the TUI client —
no fork. So the existing ancestry walk (`amon-wrapper/src/hypr.rs`: pid →
nearest ancestor with a Hyprland window, then follow `movewindow` /
`closewindow` events) works unchanged, started from the client pid instead of
a wrapper's own. That machinery moves behind a seam both the wrapper and the
daemon's herdr module can use.

Classifying `herdr` processes from `/proc` (comm is always `herdr`, argv is
never rewritten):

| role | signature |
| --- | --- |
| server | argv `…/herdr server`; session leader, no controlling tty; panes are its children; `HERDR_SESSION`/`HERDR_STARTUP_CWD` in environ |
| attached client | argv `herdr` (or `herdr --session X`, `herdr session attach X`, `herdr client`, `herdr agent attach T`); has a controlling tty |
| monolithic | argv contains `--no-session`; one process, both roles, has a tty |
| remote | `herdr --remote H` is a wrapper; its child `herdr client` owns the tty |

Session identity: `--session` argument in argv, else `HERDR_SESSION` from the
server's environ, else the default session. Cross-check when needed:
`pane.process_info` returns a pane `shell_pid` whose parent is the server pid.

Traps: just after startup the server is transiently a *child* of the client
that spawned it (classify by session-leader + no-tty, never by "clients have
no children"); debug builds use `~/.config/herdr-dev`; the server never
learns client pids (no SO_PEERCRED) and there is no `client.list` or
attach/detach event — process watching is the only, and a sufficient, route.
A detach is just the client pid exiting.

## Decisions

- **No attached client → agents are not shown.** A herdr session's agents
  enter amon's registry only while a client is attached to it, and leave
  when that client goes away. A headless server keeps its agents running and
  amon says nothing about them: there is nowhere to jump to. No "spawn a
  terminal to attach" affordance for now.

  The gate is attachment, not window resolution. A client the compositor
  cannot place — no Hyprland, over ssh, or a terminal owning several windows
  ambiguously — still projects its agents, window-less, exactly as a wrapped
  agent does in the same situation (`AgentEntry::window` is documented
  absent for precisely those cases). Hiding them here and not there would be
  two answers to one question.
- **Several clients on one session → pick the first.** Shared view means one
  agent would map to several windows; amon deterministically keeps the first
  client it resolved (and falls over to the next on its exit). Not worth more
  policy than that.
- **Movement is tracked live at both layers.** Hyprland layer: the client
  window's workspace follows `movewindow`/`closewindow` exactly as wrapped
  agents do today. herdr layer: pane and agent churn follows the event
  stream (`pane.created/closed/moved/exited`, `pane.agent_detected`, per-pane
  status subscriptions). Cross-workspace pane moves change the public pane id,
  so amon addresses agents by stable identity (`terminal_id` / agent name)
  and refreshes pane ids from `pane.moved`.
- **herdr provenance is carried on the entry.** `AgentEntry` grows optional,
  additive fields — a runtime marker (`"herdr"`), the session name, and the
  current pane target — so the panel can badge herdr agents, `amon focus` can
  do the second hop, and every existing subscriber (which already tolerates
  unknown fields/events) is untouched.

## The focus flow

`amon focus` / panel-Enter on a herdr agent is two hops: focus the client's
Hyprland window (existing path), then `agent.focus` over that session's
socket to land on the pane. The second hop flips `done → idle`, which is the
desired "you looked at it" semantics; read-only calls (`agent.list`,
`pane.read`) never mark seen, so the bar and the panel do not.

## First live run

Verified 2026-08-27 against herdr 0.8.0 (protocol 19) on this machine, with
the daemon from this branch, a named `amon-smoke` session, and a fake
`claude` in a pane:

- The process model is exactly as researched: `herdr --session amon-smoke`
  stays the tty-owning client, `herdr server` runs detached, panes are the
  server's children. Discovery, classification, and the window walk all
  landed first try — the row carried the Hyprland workspace of the terminal
  hosting the client.
- One wire deviation from the 0.8.2 docs: `session.snapshot` nests its
  payload under a `snapshot` key (`result.snapshot.agents`), where the docs
  draw it flat. `wire::agents_from` reads both.
- Protocol 19 accepted the full subscription set (`pane.agent_detected`,
  `pane.closed`, `pane.exited`, `pane.moved`, per-pane
  `pane.agent_status_changed`) without complaint.
- Status flips (`pane report-agent … --state working/blocked`) reached
  `amon status` in about a second — the event path, not the 2 s rescan.
- Detach (killing the client) removed the rows on the next rescan while the
  server kept running; reattach brought them back with herdr's preserved
  state.
- The focus hop was exercised against a fake socket only — jumping the real
  compositor's focus mid-session was not worth the surprise.
- Live use found the second half of the jump missing from the agent panel:
  it dispatched the compositor itself rather than going through
  `amon focus`, so picking a herdr agent landed on the right window with the
  wrong pane in front. That was a second implementation of "go to an agent",
  which is the thing that always drifts. Every route — `Super+N`, the bar
  click, and now the panel's pick (`amon focus --agent <id>`) — goes through
  `focus.rs`, and a test asserts no widget names an agent window
  (`address:0x`) to the compositor on its own.
- A trap this run surfaced, now resolved: on a machine with amon's shell
  aliases, typing `claude` in a herdr pane launched `amon claude` — the
  wrapper registered a window-less row (its ancestry inside herdr reaches no
  compositor window), and herdr saw a foreground process named `amon` it
  does not recognize. Asking users to type `command claude` was not an
  option, so the decision is: **inside herdr, the wrapper steps aside** —
  under `HERDR_ENV=1` it execs the agent bare. herdr detects it natively,
  its own UI and automation keep working, and this branch's projection
  carries it onto amon's surfaces. One detection authority per context:
  herdr inside herdr, amon everywhere else. (The alternative — teaching the
  wrapper to report into herdr as a custom integration — stays on the shelf
  in case wrapped-quality detection inside herdr is ever wanted.)

## A terminal outlives its agents

herdr's `terminal_id` belongs to the pane, not to what runs in it, so one
terminal can host claude, then codex, then claude again — and amon must not
date the third from the first. Two things keep them apart, and the second is
deliberately best-effort:

1. **An agent that exits leaves herdr's list.** `agent_info` returns nothing
   for a terminal that is not an agent terminal
   (`herdr/src/app/agents.rs:369`), so the pane drops out of
   `session.snapshot`, and `pane.agent_detected` — which fires on release —
   is structural here, so amon resnapshots at once and retires the row. The
   next agent arrives as a new row with its own clocks and, being a first
   claim, rings for nothing. This is the path essentially every restart
   takes.
2. **`state_change_seq` going backwards** marks a new life within one
   terminal for the case the first path misses: an exit and a restart that
   both complete between two snapshots. It is stale between snapshots (the
   status event does not carry the counter), so it catches some of that
   window rather than all of it. Closing the remainder would mean tracking
   lifecycle state through the event path, which is a lot of machinery for a
   race measured in milliseconds — reviewed and declined four times.

## What the daemon's herdr module amounts to

1. Watch for attached-client processes (pidfd on exit); resolve each to a
   window via the shared ancestry walk; follow Hyprland events.
2. Per live session: connect, `ping` (gate on protocol), subscribe globally,
   `session.snapshot`, replay, then per-agent-pane status subscriptions.
   Reconnect + re-snapshot on socket drop — live handoff swaps the server
   binary under a running session, so a drop is not "herdr exited".
3. Project herdr agents into the registry as `AgentEntry`s (ids namespaced by
   session + terminal_id), timestamping state transitions on receipt.
4. Nothing reports *into* herdr in this phase; `pane.report_agent` /
   `notification.show` / metadata tokens are later options, as is a tiny
   herdr plugin if the per-pane subscription bookkeeping ever grates.
