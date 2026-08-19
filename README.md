# amon

Know what every coding agent you are running is doing:

```sh
amon claude          # wrap any agent — output passes through untouched
amon codex --resume
amon status          # what's blocked, working, or idle right now?
amon focus 3         # go to workspace 3, landing on the agent that needs you
amon setup           # optional: agent hooks, aliases, the bar widget
amon doctor          # integration, daemon, widget, and alias health
```

`amon` runs an agent in a PTY and passes its output through unchanged, keeping
a copy for a headless *shadow terminal* it uses to detect whether the agent is
working, idle, or blocked waiting for you. It reports that to a per-user daemon
(started on demand) which keeps a live registry of every wrapped agent and
pushes events to subscribers over a unix socket.

```
$ amon status
claude   blocked    4m  3  ~/Projects/amon.sh
codex    working   12s  1  ~/Projects/scriptcast
claude   idle       3m  1  ~/Work/api
```

On Hyprland each agent also carries the window it is running in and that
window's workspace — the column before the directory — so "which one is
blocked" comes with "and it is over there". On Omarchy, `amon setup` turns that
into somewhere to go: the workspace indicators in the bar show each workspace's
most urgent agent state in place of its number, and `Super+0-9` lands on the
agent that needs your attention rather than on whatever was focused there last —
blocked first, then finished-but-unseen, then working, and never an agent at
rest. With no such agent it is the plain workspace switch, and so is every way
this can fail. Subscribers get two more facts the
terminal itself reports: whether the agent's view has focus, and whether it has
had focus since the agent last changed state, which is what tells an agent that
finished unnoticed from one you watched finish.

The agent cannot tell amon is there: amon answers no terminal queries, injects
nothing into its stream, and keeps the shadow terminal the same size as your
real one. (Amon does ask the terminal itself to report focus, and takes those
reports back out of the agent's input — [ADR-0007](./docs/adr/0007-wrapper-enables-terminal-focus-reporting.md)
covers what that costs and why.) If the daemon is missing, wedged, or killed,
the agent keeps running — observability never interrupts your session.

## Subscribing

The daemon speaks newline-delimited JSON over `$XDG_RUNTIME_DIR/amon/amond.sock`.
Any language can watch state changes live:

```sh
{ echo '{"id":"1","method":"hello","params":{"role":"subscriber","protocol":1,"version":"cli"}}'
  echo '{"id":"2","method":"subscribe"}'
  cat
} | socat - UNIX-CONNECT:$XDG_RUNTIME_DIR/amon/amond.sock
```

The wire format is documented in [docs/protocol.schema.json](./docs/protocol.schema.json),
generated from the Rust types and checked by a test.

## Building

Needs a Rust toolchain, plus **Zig 0.15.2** — the shadow terminal is Ghostty's
emulator, which builds from Zig source. With [mise](https://mise.jdx.dev) both
are pinned in `.mise.toml`:

```sh
mise install
just build
just test        # cargo-nextest; the vendored tests need a process per test
```

## Installing

One file is the whole install — the daemon and the wrapper are subcommands of
the same binary, so there is no service to enable and nothing else to place:

```sh
just install     # release build into ~/.local/bin (override with AMON_PREFIX)
amon setup       # agent hooks, aliases, and the Omarchy bar widget
amon doctor      # what is wired up and what is not
```

`~/.local/bin` because Omarchy already has it on `PATH` — its own agent
launchers live there. `amon setup` with no argument opens a screen; `amon setup
--all` takes every agent it detects plus the bar widget, and a named target is
always an agent (`amon setup claude`). Open a new shell afterwards for the
aliases.

`just uninstall` reverses it, integrations first so nothing is left pointing at
a binary that is gone.

## Credit

Agent state detection (and its manifests), the terminal state machine, and the
per-agent hook installer are derived from
[herdr](https://github.com/ogulcancelik/herdr) by Ogulcan Celik (Apache-2.0) —
an excellent agent multiplexer worth using in its own right. Detection
manifests also refresh from herdr's public catalog. Terminal emulation is
[libghostty-vt](https://github.com/ghostty-org/ghostty) (MIT).

Vendored code is never hand-edited: `just revendor` re-derives it from a pinned
herdr commit. See [NOTICE](./NOTICE) and [ADR-0005](./docs/adr/0005-vendored-code-is-never-hand-edited.md).

## Design

[CONTEXT.md](./CONTEXT.md) defines the project's language.
[docs/adr/](./docs/adr/) records the decisions that are hard to reverse — what
is vendored and why, the daemon's lifecycle, and where hooks connect.

## License

Apache-2.0. See [LICENSE](./LICENSE) and [NOTICE](./NOTICE).
