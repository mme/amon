# Ducking is one declared drop-in, and the system does the ducking

While a notification sound plays, other audio dips to a quarter volume. The
whole integration is a single file amon owns —
`~/.config/wireplumber/wireplumber.conf.d/50-amon-ducking.conf` — installed
by `amon setup` (a preselected row on the screen, `--duck`/`--no-duck`
non-interactively), removed by `amon remove`, refreshed by `--upgrade`.
Deleting it and restarting WirePlumber restores the stock audio stack, bit
for bit.

The drop-in declares two role buses (loopback sinks): Multimedia, which every
untagged stream routes through, and Notifications above it. The daemon tags
its player stream `media.role=Notification`, and WirePlumber's own role-based
policy — present but dormant in stock Omarchy until role sinks exist — ducks
the Multimedia bus while a Notification stream is live and restores it when
the stream ends. No amon process is in the loop at runtime.

## Why a permanent bus and not something lighter

Every lighter-looking design was tried or examined and lost on its failure
modes:

- **Ducking imperatively** (find every playing stream, lower it, restore it)
  touches per-app volumes that PipeWire *remembers* — a crash mid-duck leaves
  the user's music player persistently quiet. The bus is the difference
  between moving a fader amon owns and trespassing on every app's state.
- **Inserting nodes on demand** requires moving live streams, and a move is a
  hard unlink/relink — WirePlumber has no drain or crossfade anywhere in its
  linking machinery, so a gap is structural, only sometimes masked.
- **A fade on restore** (prototyped with PipeWire's volumeRampTime, and
  distinctly nicer) required either patching WirePlumber's policy script or a
  daemon in the runtime loop; both were rejected. The system's instant
  restore is the price of owning nothing at runtime.

## What it costs

Enabling role buses reroutes all audio through stereo loopbacks: surround and
bitstream-passthrough setups should skip ducking, and the docs say so. The
install restarts WirePlumber, which audibly blips playing audio — setup says
that out loud. A drop-in WirePlumber refuses would crash-loop it, so install
verifies the service is *active* afterwards (the restart's exit code lies
here) and rolls the file back if not; a foreign role-bus config in the same
directory refuses the install entirely, since fragments merge by appending.
