# Changelog

Every release, newest first. A version's section is drafted when its release
PR is prepared (`release / create-pr`), can be edited on that PR like any
other file, and becomes the GitHub Release's body verbatim when the PR
merges. The marker below is where the next section is inserted; it is part of
the machinery, not decoration.

<!-- next -->

## v0.1.1

- **The installer finishes the job**: `curl -fsSL amon.sh/install | sh` now
  flows straight into the setup screen instead of leaving you to run it
  yourself. Re-run the same command any time to update; `amon setup
  --upgrade` refreshes the desktop integrations on their own. Uninstalling
  is `amon remove --all`, then delete the binary and share directory.
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
