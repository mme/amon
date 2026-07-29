# Gaze

Gaze wraps the execution of AI coding agents (`gaze claude …`) and reports their
live state to a central per-user daemon (`gazed`), which tracks all connected
agents and notifies subscribers. Detection and install machinery is lifted from
[herdr](https://github.com/ogulcancelik/herdr) (Apache-2.0, attributed).

## Language

**Wrapper**:
The `gaze` process that spawns one agent inside a PTY, passes its I/O through
unmodified, and observes it.
_Avoid_: launcher, runner

**Daemon**:
The single per-user `gazed` process that keeps the registry of connected agents.
Started on demand by any Wrapper if not already running.
_Avoid_: server, hub

**Agent**:
An AI coding agent CLI being wrapped (claude, codex, gemini, …). Identified by
herdr's agent registry.

**Agent State**:
The detected condition of a wrapped agent: `Idle` (finished, prompt visible),
`Working` (actively processing), `Blocked` (needs human input), `Unknown`
(unrecognized program). Terminology inherited from herdr.
_Avoid_: status (reserved for the `gaze status` command), busy, waiting

**Shadow Terminal**:
An in-process terminal emulator (herdr's, lifted byte-for-byte) that consumes a
copy of the agent's output to maintain a screen grid for detection. It never
answers terminal queries — the real terminal does — and always has the same
dimensions as the real terminal.
_Avoid_: virtual terminal, headless terminal

**Detection Manifest**:
A per-agent TOML rule set (lifted from herdr) that classifies the Shadow
Terminal's screen into an Agent State.
_Avoid_: pattern file, rules file

**Integration**:
The per-agent install artifact written by `gaze install <agent>` (hook scripts,
config edits) that lets the agent itself report session events to the Daemon.
_Avoid_: plugin, extension

**Registry**:
The Daemon's live list of connected agents. An entry exists exactly as long as
its Wrapper's connection; nothing is persisted.
_Avoid_: agent list, session store

**Subscriber**:
A client connected to the Daemon to receive live notifications of agent
state changes.
_Avoid_: listener, watcher
