# amon

<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="docs/amon-logo-dark.svg">
    <img src="docs/amon-logo-light.svg" alt="" width="140">
  </picture>
</p>

amon lets you know when your agents are working, idle, or need your
attention. It runs them untouched in your own terminal. Any agent, one
keystroke away.

Built for [Omarchy Linux](https://omarchy.org).

```sh
yay -S amon-bin && amon setup      # or build from source - see below
```

## Agents as first-class citizens on your desktop

Omarchy is the perfect environment for agents: a workspace holds everything a
project needs - agent terminals, a browser, the logs - tiled or
side-scrolling, set up the way you want. amon adds the missing piece: it shows
you what your agents are doing, which one needs your attention, and lets you
jump straight to it.

Start agents in the terminal as usual. The bar shows each workspace's most
urgent agent state on that workspace's own indicator, and `Super+number` lands
on the agent that needs you rather than on whatever was focused there last -
blocked first, then finished-but-unseen, then working, never an agent at rest.

`Super+A` opens the agent panel from wherever you are, over whatever you are
doing: every agent, grouped by the workspace it is on, with what it is doing
and how long it has been at it. Pick one and Enter puts you in front of it.

There is nothing to configure. `amon setup` makes it all work automatically -
agent hooks, aliases, the bar widget, the panel, the keybindings - and
`amon doctor` reports what is wired up and what is not.

```sh
amon claude          # wrap any agent - output passes through untouched
amon codex --resume
amon status          # what's blocked, working, or idle right now?
amon focus 3         # go to workspace 3, landing on the agent that needs you
amon setup           # agent hooks, aliases, the desktop integration
amon doctor          # integration, daemon, widget, and alias health
```

```
$ amon status
claude   blocked    4m  3  ~/Projects/amon.sh
codex    working   12s  1  ~/Projects/scriptcast
claude   idle       3m  1  ~/Work/api
```

## Screenshot

![amon on Omarchy, tokyo-night theme](docs/tokyo-night.jpg)

## How it works

`amon` runs an agent in a PTY and passes its output through unchanged, keeping
a copy for a headless *shadow terminal* it uses to detect whether the agent is
working, idle, or blocked waiting for you. It reports that to a per-user daemon
(started on demand) which keeps a live registry of every wrapped agent and
pushes events to subscribers over a unix socket.

On Hyprland each agent also carries the window it is running in and that
window's workspace, so "which one is blocked" comes with "and it is over
there" - which is what the bar, the panel, and `amon focus` turn into
somewhere to go. Subscribers get two more facts the terminal itself reports:
whether the agent's view has focus, and whether it has had focus since the
agent last changed state, which is what tells an agent that finished unnoticed
from one you watched finish.

The agent cannot tell amon is there: amon answers no terminal queries, injects
nothing into its stream, and keeps the shadow terminal the same size as your
real one. (Amon does ask the terminal itself to report focus, and takes those
reports back out of the agent's input - [ADR-0007](./docs/adr/0007-wrapper-enables-terminal-focus-reporting.md)
covers what that costs and why.) If the daemon is missing, wedged, or killed,
the agent keeps running - observability never interrupts your session.

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

Needs a Rust toolchain, plus **Zig 0.15.2** - the shadow terminal is Ghostty's
emulator, which builds from Zig source. With [mise](https://mise.jdx.dev) both
are pinned in `.mise.toml`:

```sh
mise install
just build
just test        # cargo-nextest; the vendored tests need a process per test
```

## Installing

One file is the whole install - the daemon and the wrapper are subcommands of
the same binary, so there is no service to enable and nothing else to place:

```sh
just install     # release build into ~/.local/bin (override with AMON_PREFIX)
amon setup       # agent hooks, aliases, and the desktop integration
amon doctor      # what is wired up and what is not
```

`~/.local/bin` because Omarchy already has it on `PATH` - its own agent
launchers live there. `amon setup` with no argument opens a screen; `amon setup
--all` takes every agent it detects plus the desktop integration, and a named
target is always an agent (`amon setup claude`). Open a new shell afterwards
for the aliases.

`just uninstall` reverses it, integrations first so nothing is left pointing at
a binary that is gone.

## Command line reference

**`amon setup [target] [--all] [--no-alias] [--duck | --no-duck]`**

Set up integrations. Without arguments, an interactive screen; with a target,
one agent (`amon setup claude`). `--all` takes every detected agent plus the
desktop integration without a screen. `--no-alias` skips aliasing the agent's
name, on the non-interactive forms only; the screen always aliases.
Ducking - music dips while a notification plays - is included by default
where WirePlumber runs (one drop-in file amon owns; deleting it restores
stock audio); `--no-duck` opts out, and either flag alone installs or
removes just that piece.

**`amon status [--json]`**

What every connected agent is doing, most urgent first. `--json` prints the
entries machine-readable instead of as a table.

**`amon focus <workspace>`**

Go to a workspace by number, landing on the agent that needs your attention
rather than on whatever was focused there last. A plain workspace switch when
no agent wants anything.

**`amon doctor`**

Integration, daemon, widget, and alias health in one report.

**`amon --help | --version`**

Help for amon or any subcommand (`amon setup --help`), and the version. Flags
before an agent's name are amon's own; unknown ones are an error rather than
something an agent might see.

**`amon <agent> [args...]`**

Run an agent under amon. Any first word that is not a subcommand names an
agent, and every argument after it is passed to the agent verbatim, flags
included, nothing interpreted.

**`amon remove [target] [--all]`**

Remove integrations: one target, or everything after one confirmation.
`--all` skips the confirmation, for scripts.

**`amon daemon`**

Run the daemon in the foreground. Rarely needed: any amon command starts it
on demand.

**`amon hook report-agent | report-agent-session`**

Relays an installed hook's report, the agent's state or which session it is
in, to its wrapper. Used by the hook scripts `amon setup` installs, not by
hand.

## Settings

Everything configurable lives in one file, `~/.config/amon/config.toml`.
`amon setup` writes it once, fully commented and fully commented-out, and
never touches it again. Every setting is optional; the value shown is the
default. The daemon watches the file, so changes apply live under a running
bar, nothing restarts. A file that fails to parse keeps the last good
configuration (`amon doctor` reports it), and a missing file simply means
the defaults.

The glyph defaults are Nerd Font characters; they render wherever a Nerd
Font is installed, which on Omarchy is everywhere.

**`[bar]`**

- `frame_ms = 200` - how long one spinner frame is shown, in milliseconds.
  Four frames at 200ms is one turn every 800ms.
- `state_beats = 5` - the focused workspace takes turns between its agent's
  state and the marker that says you are here; beats spent on the state. A
  beat is half a spinner turn, so the rhythm follows the spinner's speed.
- `marker_beats = 3` - beats spent on the focus marker in that same
  turn-taking.

**`[bar.glyphs]`**

- `working = ["⠒", "⠰", "⠤", "⠆"]` - the animation frames for a working
  agent, in order, any length; one frame is a static glyph. Plain characters,
  here and below: a glyph setting chooses which character, never how it is
  drawn.
- `blocked = "󰋗"` - the glyph for an agent that needs input.
- `done = "󰗠"` - the glyph for an agent that finished while you were not
  looking. An agent at rest has no glyph; its workspace keeps the number,
  underlined.
- `focused = "󱓻"` - the marker for the workspace you are on.

**`[sound]`**

- `enabled = true` - a short sound when an agent starts waiting for you, and
  when one finishes unwatched. Nothing plays for an agent you are looking
  at. Set to false to silence both.
- `done = ".../finished.mp3"` - your own sound for the finished-unwatched
  case, instead of the bundled one. A relative path resolves from the config
  file's own directory.
- `blocked = ".../attention.mp3"` - your own sound for the needs-input case,
  same rules.

Give a custom sound about 300ms of leading silence: an idle sink suspends,
a USB device takes a moment to wake, and a sound without the pad loses its
opening to the DAC powering up. The bundled sounds carry it already.

Music and other audio dip to a quarter volume while these sounds play if
ducking is set up (`amon setup --duck`, on by default in setup). It is one
WirePlumber drop-in owned by amon; `amon setup --no-duck` removes it and
restores stock audio completely. The buses it routes audio through are
stereo - on a surround or bitstream-passthrough setup, skip ducking.

## Credits

- [Omarchy](https://omarchy.org) - the desktop this is built for; the bar
  widget is a four-line fork of its own workspaces widget, and the panel is
  built from its shell's components.
- [herdr](https://github.com/ogulcancelik/herdr) by Ogulcan Celik
  (Apache-2.0) - agent state detection and its manifests, the terminal state
  machine, and the per-agent hook installer are all derived from it, and
  detection manifests refresh from herdr's public catalog. An excellent agent
  multiplexer worth using in its own right.
- [libghostty-vt](https://github.com/ghostty-org/ghostty) (MIT) - terminal
  emulation, Ghostty's VT library.

Vendored code is never hand-edited: `just revendor` re-derives it from a pinned
herdr commit. See [NOTICE](./NOTICE) and [ADR-0005](./docs/adr/0005-vendored-code-is-never-hand-edited.md).

## Design

[CONTEXT.md](./CONTEXT.md) defines the project's language.
[docs/adr/](./docs/adr/) records the decisions that are hard to reverse - what
is vendored and why, the daemon's lifecycle, and where hooks connect.

## License

Apache-2.0. See [LICENSE](./LICENSE) and [NOTICE](./NOTICE).
