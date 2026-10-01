# Agent events ride the terminal stream, knock first

A wrapper on the far side of an ssh session has no socket that reaches this
machine, but its terminal bytes arrive faithfully — through OpenSSH,
Tailscale SSH, jump-host chains, anything that carries an interactive
session. So remote agents become local rows by whispering: the remote
wrapper mirrors its own daemon-link traffic (`agent.register`,
`agent.update`) into its stdout as private DCS sequences
(`ESC P +amon;1; … ESC \`), and the wrapper on this side strips them back
out of the passthrough, before the real terminal or the Shadow Terminal
sees a byte of them, and replays them against the local daemon. Hook
Authority never crosses the wire at all — hooks report to the wrapper next
to the agent, exactly as they do locally, and only the arbitrated result
travels.

The trade-off was:

- **A second connection** (`ssh host amon pipe`, port forwarding, a
  daemon-to-daemon bridge): keeps the terminal stream pure, but demands
  more of the transport than "carries terminal bytes" — remote forwarding
  is exactly what restricted SSH servers do not offer — and needs its own
  lifecycle, auth story, and reconnect machinery.
- **Whisper unconditionally**: simplest, but sprays escape sequences —
  carrying cwd, argv, and branch names — into every recording, `script`
  log, and `TERM=dumb` buffer of every ssh'd session, listener or not.
- **Whisper after a knock** (chosen): at startup, only inside an ssh
  session (`SSH_TTY`), the remote wrapper emits one ~20-byte probe that a
  conforming terminal discards silently. A local wrapper that hears it
  answers down the input stream — the channel ADR-0007 already filters —
  and only then does the stream carry events. No answer, no second
  sequence, ever.

This bends the "output passes through untouched" promise the way ADR-0007
bent input purity, and by a measured amount: for a session with no amon
listening, the entire cost is one discardable sequence; for a listening one,
both ends are amon and nothing else ever sees the whispers. The envelope is
versioned (`+amon;1;`) and additive-forever like the socket protocol, the
local parser treats the remote host as untrusted (malformed or oversized
frames are discarded, never surfaced, never allowed to desynchronize the
passthrough), and every consumer — bar, panel, sounds, `amon focus` — works
on mirrored rows unchanged, because a mirrored row is an ordinary registry
entry whose `window` and `workspace` are grafted from the ssh session's own.

## Consequences

Anything that consumes escape sequences between the two wrappers breaks the
channel invisibly: tmux on the remote end swallows unknown DCS unless
wrapped in its passthrough envelope (a possible v2), and mosh re-renders
the screen and drops them — both documented limits, both degrading to
today's behavior, an `Unknown` row with screen detection of whatever is
visible. The whisper's liveness is the stream itself: a heartbeat register
rides along at most every 30 seconds, and 90 seconds of silence reverts the
row.
