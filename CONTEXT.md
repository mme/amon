# Amon

Amon wraps the execution of AI coding agents (`amon claude …`) and reports their
live state to a central per-user daemon (`amond`), which tracks all connected
agents and notifies subscribers. Detection and install machinery is lifted from
[herdr](https://github.com/ogulcancelik/herdr) (Apache-2.0, attributed).

## Language

**Wrapper**:
The `amon` process that spawns one agent inside a PTY, passes its I/O through
unmodified, and observes it. In two contexts it steps aside instead — `exec`s
the agent and wraps nothing: inside a herdr pane, where herdr is the detection
authority, and where it has a terminal on neither side, which means there is no
window for a row to lead to (ADR-0016).
_Avoid_: launcher, runner

**Daemon**:
The single per-user `amond` process that keeps the registry of connected agents.
Started on demand by any Wrapper if not already running.
_Avoid_: server, hub

**Agent**:
An AI coding agent CLI being wrapped (claude, codex, gemini, …). Identified by
herdr's agent registry.

**Detected Agent**:
An Agent whose CLI the registry finds on this machine. The set `amon setup`
offers and `--all` covers; a supported but undetected Agent can still be named
explicitly.
_Avoid_: available agent, installed agent (installed describes Integrations,
not Agents)

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
What `amon setup` writes into something else's configuration, in either
direction: an *agent* Integration (hook scripts, config edits) lets the
agent report session events to the Daemon, while a *desktop* Integration is a
Subscriber amon ships, consuming those events rather than producing them.
Either way amon owns the artifact and reports whether the installed copy is
current. Never the amon binary itself — getting that onto the machine is the
package manager's job.
_Avoid_: plugin, extension (what the host calls it is the host's word)

**Alias**:
The bash alias amon installs for an Agent — `alias claude='amon claude'` — so
that running the Agent by its own name runs it under the Wrapper without anyone
typing `amon` first. Interactive shells only; the Shim covers the launch path
that never sees a shell.
_Avoid_: launcher, shim (that is the other mechanism, below)

**Shim**:
The stand-in executable amon writes to `~/.local/share/amon/shims/<agent>`,
which `~/.config/hypr/amon.lua` puts ahead of the real binary on PATH — so an
Agent launched without a shell, as Omarchy's default-agent launcher does
through a terminal's `-e`, still runs under the Wrapper (ADR-0013). Written
only for agents you set up, and only where Omarchy is; the third thing an
integration owns, beside its hooks and its Alias, and audited by `amon doctor`
like the other two. Unqualified, "shim" means this; the crate's
vendor-compatibility layer is always "the vendor shim".
_Avoid_: wrapper script (the Wrapper is the process, not the file)

**Registry**:
The Daemon's live list of connected agents. An entry exists exactly as long as
its Wrapper's connection; nothing is persisted.
_Avoid_: agent list, session store

**Subscriber**:
A client connected to the Daemon to receive live notifications of agent
state changes.
_Avoid_: listener, watcher

**Agent Panel**:
The desktop surface listing every Agent the compositor can show — Super+A's
centred modal, and the floating window it can pop out into. Both hosts draw
one shared pane (`AgentsView`), one Subscriber between them, so they cannot
drift into two views of one thing. "Panel" is the surface, "pane" is the view
inside it; a row chosen in either goes to that Agent's Window.
_Avoid_: agents list, popup, dialog (the modal is one host of it, not the
thing itself)

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

**Done**:
An Agent that has finished and not been looked at: `Idle` and not Seen. Not an
Agent State — the states stay herdr's — but the condition amon exists to
surface, so it is named rather than spelled out at every use.
_Avoid_: finished, complete, unread

**Attention**:
The one ranking of what wants a human, most first: Blocked, Done, Working,
Idle, with `Unknown` last. Computed over an Agent Entry rather than an Agent
State, because Done is not a state. It sorts `amon status`, decides which state
a workspace indicator draws, and decides where `amon focus` lands — one
function, so the bar and the keystroke cannot disagree (ADR-0011).
_Avoid_: urgency, priority, severity
