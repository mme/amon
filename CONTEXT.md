# Amon

Amon wraps the execution of AI coding agents (`amon claude …`) and reports their
live state to a central per-user daemon (`amond`), which tracks all connected
agents and notifies subscribers. Detection and install machinery is lifted from
[herdr](https://github.com/ogulcancelik/herdr) (Apache-2.0, attributed).

## Language

**Wrapper**:
The `amon` process that spawns one agent inside a PTY, passes its I/O through
unmodified, and observes it.
_Avoid_: launcher, runner

**Daemon**:
The single per-user `amond` process that keeps the registry of connected agents.
Started on demand by any Wrapper if not already running.
_Avoid_: server, hub

**Agent**:
An AI coding agent CLI being wrapped (claude, codex, gemini, …). Identified by
herdr's agent registry.

**Agent State**:
The detected condition of a wrapped agent: `Idle` (finished, prompt visible),
`Working` (actively processing), `Blocked` (needs human input), `Unknown`
(unrecognized program). Terminology inherited from herdr.
_Avoid_: status (reserved for the `amon status` command), busy, waiting

**Shadow Terminal**:
An in-process terminal emulator (herdr's, lifted byte-for-byte) that consumes a
copy of the agent's output to maintain a screen grid for detection. It never
answers terminal queries — the real terminal does — and always has the same
dimensions as the real terminal.
_Avoid_: virtual terminal, headless terminal

**Effective State**:
The Agent State the Wrapper reports onward, arbitrated from two sources: the
Shadow Terminal's screen signals and Hook Authority. Only the Wrapper computes
it; the Daemon stores what it is told.
_Avoid_: resolved state, final state

**Hook Authority**:
Agent State (or session identity) reported by an installed Integration's hooks,
as opposed to derived from the screen. One input to Effective State.
_Avoid_: hook state, integration state

**Detection Manifest**:
A per-agent TOML rule set (lifted from herdr) that classifies the Shadow
Terminal's screen into an Agent State.
_Avoid_: pattern file, rules file

**Integration**:
What `amon install <target>` writes into something else's configuration, in
either direction: an *agent* Integration (hook scripts, config edits) lets the
agent report session events to the Daemon, while a *desktop* Integration is a
Subscriber amon ships, consuming those events rather than producing them.
Either way amon owns the artifact and reports whether the installed copy is
current.
_Avoid_: plugin, extension (what the host calls it is the host's word)

**Alias**:
The bash alias amon installs for an Agent — `alias claude='amon claude'` — so
that running the Agent by its own name runs it under the Wrapper without anyone
typing `amon` first. Interactive shells only: an Agent started any other way
runs unwrapped, exactly as it would with no Alias at all.
_Avoid_: launcher, shim (the second is taken: the crate's vendor-compatibility
layer)

**Registry**:
The Daemon's live list of connected agents. An entry exists exactly as long as
its Wrapper's connection; nothing is persisted.
_Avoid_: agent list, session store

**Subscriber**:
A client connected to the Daemon to receive live notifications of agent
state changes.
_Avoid_: listener, watcher

**Window**:
The compositor window hosting the agent's terminal, held as an opaque
compositor-native token. Mapped by the Wrapper; meaningful only when handed
back to the compositor (e.g. to focus it). Absent when there is no compositor
or the mapping is ambiguous — never guessed.
_Avoid_: client (Hyprland jargon), terminal (that's the emulator)

**Workspace**:
The compositor workspace currently holding the Window, as a display name.
Tracked live; changes when the Window moves.
_Avoid_: desktop, tag

**Focus**:
Whether the agent's terminal *view* has input focus, learned from terminal
focus reporting — not from the compositor. A focused Window can host many
views (tabs, panes); Focus is per-view. Unknown when there is no terminal to
report it; on one, taken as focused at launch until the terminal says
otherwise.
_Avoid_: active, compositor focus

**Seen**:
Whether the agent's terminal view has had Focus at any point since the
current Agent State began. Reset at every state change (immediately true if
focused at that instant). Computed by the Wrapper. `Idle` and not Seen is
the "finished but unnoticed" condition.
_Avoid_: unread, acknowledged, notified
