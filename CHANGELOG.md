# Changelog

Every release, newest first. A version's section is drafted when its release
PR is prepared (`release / create-pr`), can be edited on that PR like any
other file, and becomes the GitHub Release's body verbatim when the PR
merges. The marker below is where the next section is inserted; it is part of
the machinery, not decoration.

<!-- next -->

## v0.3.0

- **Agents in luvus show up**: amon watches for
  [luvus](https://github.com/RizRiyz/luvus) sessions the way it does herdr's
  — their agents join the bar and the panel, `amon focus` lands on the pane,
  and `amon <agent>` inside a luvus pane runs the agent bare.
- **`amon focus` hops to the pane even without a window**: an agent in a
  session the compositor never placed (over ssh, no Hyprland) still gets the
  half of the jump amon can make.
- **Protocol**: `AgentEntry.herdr` is now `AgentEntry.runtime`, tagged by
  `kind` (`herdr` | `luvus`). Nothing outside amon read the old field.
  `AMON_HERDR=0` is now `AMON_RUNTIMES=0`.

## v0.2.0

- **Agents in herdr show up**: amon watches for
  [herdr](https://github.com/herdrdev/herdr) sessions and puts their agents
  on the bar and in the panel beside the wrapped ones, with herdr's own
  states, for as long as a client is attached. Jumping to one lands on its
  pane. Inside a herdr pane, `amon <agent>` runs the agent bare, so herdr
  stays the one detecting it. A session that reconnects keeps its one row
  instead of spawning a duplicate, and doesn't chime for a state it already
  left.
- **Rows lead with where an agent is working**: the panel and `amon status`
  now put the project (or directory) first and in bold, with the branch, how
  long it's been in its state, and the agent kind following in the same
  order on both surfaces. An agent's project and branch update live as it
  moves around, including when it's dispatched into a worktree.
- **No more rows with nowhere to jump**: an agent started with no terminal
  behind it - in the background, from a script, or by another agent - no
  longer takes a panel row that nothing can focus; amon now runs it bare
  instead.
- **Fewer misdirected agents**: fixes a bug where an agent started by
  another wrapped agent could end up unwrapped and reporting its state onto
  the wrong row, and a bug where a home directory containing brackets or
  wildcards sent the wrapper into an infinite loop instead of starting the
  agent.
- **Detection keeps up with Claude Code**: re-vendors herdr's detector,
  picking up recognition of Claude Code's newer busy spinner and four
  months of other upstream agent-detection fixes.
- **Upgrading amon shows the new panel**: fixes a bug where running
  `amon setup` again could leave the shell drawing the pre-upgrade panel;
  amon now restarts the shell itself so Super+A opens what was just
  installed.

## v0.1.1

- **Chimes survive a sleeping speaker**: amon now wakes a suspended audio
  device itself before playing a chime, instead of hoping it's already
  awake. Fixes chimes getting clipped or lost on their first note,
  especially over Bluetooth.

## v0.1.0

The first release. amon runs your coding agents untouched in your own
terminal and tells you when they are working, idle, or need you - built for
Omarchy Linux.

- **Wrap any agent**: `amon claude`, `amon codex`, or the alias `amon setup`
  writes for you. Output passes through byte for byte; a shadow terminal
  reads the agent's state without the agent ever noticing.
- **The bar knows**: each workspace's indicator shows its most urgent
  agent's state - blocked, finished unseen, working - and `Super+number`
  lands on the agent that needs you.
- **One panel for everything**: `Super+A` lists every agent with what it is
  doing and for how long; Enter puts you in front of it. `Super+W` closes
  the panel, not the window behind it.
- **Sounds that respect your music**: a chime when an agent finishes
  unwatched or starts waiting, and with ducking set up, other audio dips
  underneath it - one WirePlumber drop-in, removed as cleanly as it came.
- **Stays current**: the daemon notices a newer release once a day and the
  panel shows the one-line upgrade command; nothing updates itself.
- Install: `curl -fsSL amon.sh/install | sh` - one binary into
  `~/.local/bin`, no root, sha256-verified, straight into the setup screen.
