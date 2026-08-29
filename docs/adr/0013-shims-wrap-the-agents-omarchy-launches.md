# Shims wrap the agents Omarchy launches, for exactly the agents you set up

`amon setup` wraps an agent by aliasing its name in `~/.bashrc` (ADR-0009).
That covers agents you start by typing their name. It does not cover the one
Omarchy starts for you, and the reason is structural rather than a bug:

```
SUPER+SHIFT+CTRL+A  →  omarchy-agent --pick
    command=(claude --permission-mode bypassPermissions)
    exec omarchy-launch-tui --app-id=org.omarchy.agent "${command[@]}"
        exec setsid uwsm-app -- xdg-terminal-exec --app-id=… -e "$1" "${@:2}"
            foot -e claude --permission-mode bypassPermissions
```

The terminal `exec`s the binary directly. There is no shell, so there is no
interactive `.bashrc`, so there is no alias. The menu reaches the same script by
a different door: `omarchy-default-agent <name>` ends in `exec omarchy-agent`.

## Why not shadow `omarchy-agent` itself

It is the single choke point for both doors, which makes it the obvious target,
and it can be shadowed — the session `PATH` is ours to prepend to. It is
rejected for what it would cost rather than for being impossible.

Wrapping it directly does not work: it `exec`s a detaching terminal launcher and
exits, so amon would wrap the launcher and the agent would run in a new terminal
outside amon's PTY. The way through is `omarchy-agent --inline`, which `exec`s
the agent in the current process — but then amon must be told *which* agent it
is wrapping, because it takes the name from `basename(argv[0])` and would
register an agent called `omarchy-agent`, matching no detection manifest. That
means a new `--as` flag, plus reimplementing the terminal launch (the
`omarchy-launch-tui` call and its `--app-id`), plus a wart: with no default set,
`--pick` `exec`s the menu, which would now open behind an already-spawned empty
terminal.

Shadowing the *agent* names costs none of that. Omarchy resolves the agent by
name at runtime, so the interception happens inside `omarchy-agent` rather than
in front of it — which is also why it covers the menu for free — and every piece
of upstream's logic stays upstream's: the nine-agent flag table, the `cd
~/Work`, `--prompt`, `--pick`.

## The set is the set you already chose

A shim directory holding *every* agent would wrap agents you never asked amon to
touch. Instead it holds exactly what `amon setup` wrapped, from the same
`command_names()` that drives the aliases — so `cursor` becomes `cursor-agent`
and Kilo gets both its names, without a second table to keep in step.

| | interactive shell | compositor session |
|---|---|---|
| mechanism | `alias claude='amon claude'` | `~/.local/share/amon/shims/claude` |
| set | `command_names()` of set-up targets | identical |
| installed / removed by | `amon setup` / `amon remove` | identical |

An agent you never set up is untouched: Omarchy launching it runs it bare,
exactly as before amon existed. This is ADR-0009's promise — amon takes over the
name of the agents you asked it to — extended from the shell to the session,
rather than a second, wider policy.

## Removing one agent removes its shim

Not just `amon remove`. The shim is the third thing an integration owns, beside
its hooks and its alias, and it has to come off wherever those do:

- **`Action::RemoveAgent`** already removes hooks and the alias line; the shim
  joins them there. That one action serves both `amon remove <agent>` and
  unchecking a row on the setup screen, so both get it without a second path.
- **Remove-everything** sweeps the directory whole, which also catches shims
  whose integration has already gone — the same orphan case `alias::stranded`
  exists for, and for the same reason: a shim left behind keeps taking over its
  agent's name long after the integration that justified it stopped existing.
- **`amon doctor`** reports shims the way it reports aliases. An integration
  whose alias is live but whose shim is missing means the agent is wrapped in a
  terminal and bare from the keybinding, which is exactly the split this ADR
  exists to close, and it should be visible rather than deduced.

Removal is simpler than the alias case and should not borrow its defensiveness.
A shim is a file amon wrote in a directory amon owns, so there is no symlinked
rc to refuse and no fence to parse: it deletes, or it is a real error. The
`alias::uninstall`-returns-Ok-while-refusing dance has no equivalent here.

**The directory and the `PATH` entry are one feature.** The entry is written by
`~/.config/hypr/amon.lua`, which only exists where Omarchy does, so shims are
installed under the same condition as the bindings and removed with them — and
the directory goes when the last shim does. Writing shims on a machine whose
`PATH` will never include them would be inert files pretending to be an
integration.

## What the measurements forced

The directory is prepended to `PATH` from `~/.config/hypr/amon.lua`, the file
amon already owns and already removes (ADR-0011), because `hyprland.lua` loads
Omarchy's defaults before the user's modules. Three things were measured on a
live session rather than assumed, and two of them changed the design.

**`hl.env` accumulates, and the config sees its own past.** `os.getenv("PATH")`
inside the config returns the value earlier `hl.env` calls produced — Hyprland's
own environment is mutated. So the naive `hl.env("PATH", ours .. ":" ..
os.getenv("PATH"))` appends a copy *on every config reload*: four reloads, four
copies, measured. Omarchy's own `envs.lua` strips its directory out before
re-inserting it at the front, and that loop is not style — it is the only reason
`omarchy/bin` appears once. Amon must do the same, and the shim directory has to
be removed from the string before it is prepended.

**It does reach the processes that matter.** A Hyprland-dispatched command sees
the modified `PATH`, through `setsid`, `uwsm-app` and `xdg-terminal-exec` — so
the interception works where it needs to.

**Removing the line does not undo it.** Deleting the `hl.env` call and reloading
leaves the entry in Hyprland's environment; only setting `PATH` again, or a new
session, clears it. So `amon remove` cannot fully clean this until next login.
Harmless — the directory it names is gone, and a `PATH` entry pointing at
nothing is skipped — but it means removal is complete on disk and lagging in one
long-running process, which is worth saying out loud rather than discovering.

## Consequences

**The shim must not call itself.** `amon claude` spawns `claude` through
`CommandBuilder`, which resolves it on `PATH` — and finds the shim. Stripping
the directory inside the shim terminates the loop but still wraps twice: two
amon processes, two registry entries for one agent. The guard is the environment
the wrapper already sets for its child: a shim that sees `AMON_AGENT_ID` knows
it is being called *by* amon and `exec`s the real agent instead.

**A shim is a second thing to keep in step with `command_names()`.** Aliases and
shims must always name the same commands, or an agent is wrapped in a terminal
and bare from the keybinding. One list, two writers, and a test that walks both.

**This is amon reaching further into the session than before.** Aliases only
ever affected shells the user started. A `PATH` entry affects everything
Hyprland launches — scoped to chosen agents, but no longer scoped to shells.
`amon remove` is the whole undo, and it should stay that way.

**Upstream would make all of it unnecessary.** One line in `omarchy-agent` —
`command=(${OMARCHY_AGENT_WRAPPER:+$OMARCHY_AGENT_WRAPPER} "${command[@]}")` —
would cover every door with no shims, no `PATH` edit and no keybinding
ownership, and would serve herdr equally. `omarchy-agent` is a packaged script
rather than a user file, so such a hook would reach every Omarchy user at once.
Worth filing regardless of what is built here; if it lands, this ADR's mechanism
is deleted rather than maintained.

## Postscript: the guard compares a pid, not a mark

"A shim that sees `AMON_AGENT_ID` knows it is being called *by* amon" was too
strong a reading of that variable. It is inherited by everything the agent
starts, so a shim reached from inside a wrapped agent saw it too and stepped
aside for an agent amon had never wrapped — which then ran unwrapped while still
carrying the outer agent's id and socket, and reported its state into the outer
agent's row.

The wrapper now also sets `AMON_WRAPPER_PID` to its own process id, and the
guard fires only when that equals the shim's `$PPID`: amon is the caller, not
merely somewhere above. ADR-0016 has the reasoning and the residual risk.
