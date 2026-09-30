# Changelog

Every release, newest first. A version's section is drafted when its release
PR is prepared (`release / create-pr`), can be edited on that PR like any
other file, and becomes the GitHub Release's body verbatim when the PR
merges. The marker below is where the next section is inserted; it is part of
the machinery, not decoration.

<!-- next -->

## v0.5.0

- **The Creator Micro 2 joins the desk**: Work Louder's macropad becomes a
  hardware panel for your agents. Its six keys mirror the agent panel's
  rows and light with each agent's own state - tap one to jump straight to
  that agent, exactly like the panel does, even when the agent lives inside
  a herdr or luvus pane. The ring glows the fleet's most urgent color and
  taps to focus whichever agent needs you. The encoder scrolls the agent
  panel or drives its selection, and once you have more than six agents it
  walks the rest of the list; the joystick moves window focus. Seven macro
  keys are yours to bind under `[devices.micro2]` - the second one defaults
  to starting a new agent (Omarchy's own agent picker), the rest ship
  blank.
- **Hold a macro key to dictate**: a tap still toggles dictation as before;
  a hold is push-to-talk, ending in voxtype's own auto-submit so the text
  is typed and entered together. Music now ducks to silence for exactly as
  long as the microphone is live - the mechanism that actually works,
  since stock Omarchy's own media-pausing turns out to be broken. Tunable
  under `[devices.micro2.dictation]` (`hold_ms`, `auto_submit`).
- **Setup and doctor understand the keypad**: `amon setup` writes the one
  udev rule the device needs and can write amon's own agent layer onto the
  keypad's firmware, telling you which layer to tap the touch sensor to.
  `amon doctor` now checks the hardware itself - firmware version, whether
  the agent layer is active, whether the device and its virtual keyboard
  are reachable - instead of assuming a device it can open is already lit.
- **The agent panel orders rows by where they sit on screen**: left to
  right, then top to bottom, instead of by when the agent started. The
  Micro 2's keys follow the same order, so key N is always the panel's row
  N.
- **Fix**: a key press (or anything else that spawns `amon focus`) could
  silently do nothing right after an update, because amon looked for its
  own binary at a path the installer had just replaced.
- **Protocol**: `AgentEntry` gains `position`, reported by the compositor
  crate and kept in step between the panel and the Micro 2 module.
  `docs/protocol.schema.json` is updated.
- The README, website and a new ADR document the whole path - firmware
  floor, udev rule, agent layer, dictation settings - with a complete
  config example.

## v0.4.0

- **The row says what the agent is doing**: every panel row now carries the
  agent's own words for its current step - "Reading 1 file…" or
  `Bash(cargo nextest run)` while it works, the opening line of its reply
  once it's done, the question it's waiting on when it's blocked - and the
  prompt you gave it, in italic behind the agent's own marker, so you can
  tell which turn you're looking at. Never a phrase amon composed. A working
  row's message shimmers to mark it as live. The kind and state columns are
  gone: the glyph already carries the state, and the message takes the width
  they used. When the pane narrows, a long message loses its tail rather than
  the project or branch that names the row.
- **Every agent narrates, through whatever channel it offers**: Claude,
  Codex and grok report the prompt through their own hook engines and their
  narration off the screen; pi, omp and opencode report prompt, tool calls
  and replies through their own extension or plugin APIs. A blocked pi or omp
  row now shows the question that blocked it instead of just "blocked".
- **Upgrading**: run `amon setup <agent>` again to install the new hooks and
  extensions; `amon doctor` names any that are absent or stale. The shell
  hooks parse their input with `python3`, so a machine without it sees no
  prompt on the row.
- **Agents in herdr and luvus panes narrate too**: inside a runtime pane,
  `amon <agent>` now wraps the agent instead of stepping aside, adds what
  it's doing to the row the runtime already owns, and leaves state and focus
  to the runtime. It presents itself under the agent's own name, so herdr
  and luvus still see `claude`, not `amon`.
- **`amon doctor` catches an integration with no alias**: skip the alias
  `amon setup <agent>` writes and that agent runs bare, never reaching the
  bar or the panel, which looks like amon is broken. Doctor now says so and
  points at `amon setup <agent>`.
- **Protocol**: `AgentEntry` gains `activity` (`text`, `kind`); the wrapper
  socket accepts `report_activity`, and the daemon accepts `runtime.activity`
  from a wrapper inside a runtime pane. `docs/protocol.schema.json` is
  updated.

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
