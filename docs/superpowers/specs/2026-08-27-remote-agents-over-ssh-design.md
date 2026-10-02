# Remote agents over SSH

Design, 2026-08-27. Status: draft for review.

`amon ssh <host>` makes an agent running on another machine a first-class
citizen of the local desktop: it appears in the bar, the panel, and
`amon status`, sounds fire when it blocks or finishes unwatched, and
`Super+number` lands on it — because "it" is the ssh session's window, which
is right here. SSH is the only transport, and the design requires nothing of
it beyond carrying terminal bytes, so Tailscale SSH, jump-host chains, and
restricted servers all work identically.

## Goals and non-goals

**Goals**

- A remote agent shows up in the local registry with everything a local one
  has: agent kind, state, cwd, branch, title, session identity, hostname.
- Hook Authority works remotely: the remote agent's hooks report exactly as
  they do locally, and the arbitration happens where it always does, in the
  wrapper next to the agent.
- The transport requirement is "an interactive SSH session exists." No port
  forwarding, no `-R`/`-L`, no extra connections, no listening sockets.
- amon runs headless on macOS so a Mac can be the remote end.

**Non-goals**

- Persistence. The remote agent's lifetime is the ssh session's lifetime,
  exactly as a local agent's is its terminal window's. Detach/reattach is
  tmux's or herdr's job, or a later amon layer — not this one.
- Federation. This design sees the agents inside sessions you opened, not a
  remote host's whole registry. A daemon-to-daemon bridge (`ssh host amon
  pipe`) remains open as a later, complementary layer.
- mosh. It re-renders the screen from its own emulator and drops unknown
  sequences; documented as unsupported.
- A macOS desktop UI. Headless means wrapper, daemon, and CLI.

## Shape

Two wrappers, one stream:

```
remote:  hook ──unix socket──▶ amon wrapper (wraps claude) ──▶ remote daemon
                                    │
                                    └──▶ DCS whisper into its own stdout
                                              │
                            PTY → sshd → SSH channel → ssh client
                                              │
local:   amon ssh host  ── wrapper (wraps ssh) strips the whisper,
                           mirrors the entry into the local daemon
```

On the remote host nothing changes about how agents are run: the alias makes
`claude` run `amon claude`, the wrapper computes Effective State from the
shadow terminal and Hook Authority, and reports to the remote daemon as
always. Additionally — only in an ssh session, only after a knock is
answered — the remote wrapper writes its registry traffic into the terminal
stream as a private DCS sequence.

Locally, `amon ssh` wraps the ssh client the way `amon claude` wraps claude.
Its stream scanner recognizes the whisper, strips it before the real terminal
and before the shadow terminal, and converts it into `agent.register` /
`agent.update` against the local daemon — existing methods, no daemon or
protocol change. Every consumer (bar, panel, sounds, `amon focus`,
`amon status`) sees an ordinary entry and needs no code for any of this.

The precedent for both halves is already shipped: the shadow terminal parses
every output byte, and ADR-0007's machinery already strips recognized
sequences (focus reports) out of the input stream. The whisper is the same
trick on the output side.

## The knock

The remote wrapper must never emit sequences nobody strips: they would end up
in recordings (`script`, asciinema, tmux logs), in `TERM=dumb` and Emacs
shell buffers as visible junk, and they carry cwd, argv, and branch — not
something to leak into every captured session. So:

1. At startup, when `SSH_TTY` is set (and only then), the remote wrapper
   emits **one** probe sequence, ~20 bytes, and otherwise stays byte-identical
   to today. A conforming terminal discards it silently; that one sequence is
   the entire cost in a session with no local amon.
2. A local `amon ssh` on the other end answers with one sequence down the
   input stream — the channel focus reports already arrive on, filtered by
   the machinery ADR-0007 built.
3. Only after the answer does the remote wrapper stream events: a full
   `agent.register` first, then `agent.update` on change. No answer, no
   second sequence, ever.

Event volume is noise-floor: updates are sent only when the entry actually
changes (a state flip, a title or branch change, focus in/out), one short
JSON line each, against an agent stream of kilobytes per second.

**Sequence format.** A private DCS envelope with a versioned prefix and a
JSON payload (the same frames the wire protocol uses). Both ends are amon, so
no terminal's length limits apply — the sequences are stripped before any
real terminal sees them. Exact bytes are an implementation detail; the
constraints are: unambiguous prefix, ST-terminated, payload that cannot
contain the terminator, version field for the same additive-forever
discipline as the socket protocol.

**Parsing posture.** The local scanner parses bytes from a machine that may
be hostile. Malformed or oversized sequences are discarded, never panicked
on, and can never desynchronize the passthrough — the same trust the shadow
terminal already extends to agent output. A fabricated event's blast radius
is a wrong row in the bar and a focus jump onto the ssh window the user would
have jumped to anyway.

**Known race.** If the remote wrapper exits between knock and answer, the
answer's ~20 bytes reach the remote shell as typed input. The window is
milliseconds; the local side narrows it further by answering only while the
session that carried the knock is still alive. Accepted.

## Field semantics at the boundary

The whisper carries the complete `AgentEntry` verbatim. The local wrapper
reinterprets a few fields as it mirrors:

| Field | Treatment |
| --- | --- |
| `agent`, `state`, `cwd`, `args`, `branch`, `title`, `agent_session_id`, `agent_session_path` | Pass through untouched. |
| `window`, `workspace` | Absent remotely (no compositor behind sshd). The local wrapper grafts **its own** window and workspace onto the entry — this is what makes `Super+number`, the bar indicator, and the panel's Enter work on remote agents with zero consumer changes. |
| `focused`, `seen` | Pass through. Computed remotely, and correctly: the real terminal's focus reports already travel down the ssh input stream, so ADR-0007's machinery on the remote wrapper sees them natively. "Finished but unnoticed" works across the wire for free. |
| `state_since`, `started_at` | Re-stamped by the local wrapper on receipt. The agent lives inside this session, so every transition is observed as it happens; re-stamping loses nothing and removes remote clock skew as a topic. |
| `hostname` | Pass through (the remote fills it), but the local wrapper records the ssh destination alongside — a host that calls itself `localhost` does not get to label the panel. Presentation (e.g. `host:~/path`) is a consumer decision, later. |
| `pid` | Pass through, informational only. It is a remote pid; nothing local may treat it as actionable. |
| `id` | Pass through, namespaced by the local wrapper so a remote id cannot collide with a local one. |

## One session, one row

Today, wrapping `ssh` would register an `Unknown` entry for the ssh process
itself. Once the knock is answered, the local wrapper stops being an agent in
its own right: its registry entry *becomes* the mirrored remote entry
(replacing the `Unknown` registration), and follows the remote agent's
lifecycle from then on. If the remote agent exits but the ssh session
continues, the wrapper reverts to its own `Unknown` entry; if a new remote
agent knocks in the same session, the cycle repeats. One session, one row,
always.

Sessions where nothing ever knocks — no amon on the remote host, or an amon
older than this feature — behave exactly as today: an `Unknown` row, with the
local shadow terminal's screen detection still classifying whatever is on
screen. Degradation is the status quo, which is the compatibility bar the
protocol has always held itself to.

## Failure modes and limits

- **tmux on the remote end** swallows unknown DCS unless the sequence is
  wrapped in tmux's passthrough envelope (and `allow-passthrough` is on).
  v1 documents this as a limit; a v2 may wrap when `$TMUX` is set. Losing
  the whisper is invisible — the session simply stays an `Unknown` row.
- **Chained hosts** (amon ssh → host A → amon ssh → host B): bytes pass
  faithfully, and A's wrapper strips B's events and mirrors them into A's
  daemon. Re-emitting upward so the outermost desktop sees B is deliberately
  out of scope; noted for later.
- **Lifecycle is the session.** ssh drops — lid closed, network gone — the
  remote PTY hangs up, the agent dies, the entry vanishes with the wrapper's
  connection, exactly as closing a local terminal window. There is no
  reconnect protocol and no staleness model, on purpose.

## Headless macOS

The remote end must run the wrapper and daemon, and the user wants the CLI —
`amon status`, `amon setup` (agent integrations), `amon doctor` — on the Mac.
The crate boundary already is the feature boundary:

- **Portable today:** `amon-wrapper` (portable-pty, POSIX termios via libc),
  `amon-term` (libghostty-vt — Ghostty is native Mac software, the Zig build
  is mac-first), `amon-protocol` (`paths.rs` already handles macOS's 104-byte
  socket limit), `amon-daemon`, `amon-detect`. No `cfg(target_os)` exists in
  amon's own crates.
- **Linux-only:** `amon-integration` (Hyprland, the QML panel, WirePlumber
  ducking, udev) and the desktop halves of `setup` and `doctor`.

Work items:

1. Gate `amon-integration` (and the desktop arms of `setup`/`doctor`/`focus`)
   out of macOS builds; on macOS, `setup` offers agent hooks and aliases
   only, `doctor` audits only what can exist there, and `amon focus` is
   absent.
2. Sound: the daemon's notification sounds are desktop concerns; headless
   macOS ships without them (a remote agent's sounds play on the *local*
   desktop via the mirrored entry, which is the point).
3. Release CI builds `aarch64-apple-darwin`; `install.sh` picks the target
   by `uname`.
4. Aliases target the user's actual shell; macOS defaults to zsh, which the
   alias installer must handle if it does not already.

## Testing

- The whisper channel is just bytes, so the knock, the strip, and the
  mirroring are all testable through PTY pairs in the existing e2e harness
  (`crates/amon-cli/tests/e2e.rs`), no real ssh needed: a fake "remote
  wrapper" writes probe and events into the stream `amon ssh`'s wrapper
  reads.
- Field-boundary behavior (grafting, re-stamping, id namespacing, the
  become-a-row / revert cycle) unit-tests against the mirroring layer.
- Strictness: fuzz-shaped tests feeding truncated, oversized, and malformed
  sequences through the scanner, asserting the passthrough stream is
  byte-identical minus well-formed whispers.
- One optional loopback-ssh integration test (`ssh localhost`) gated on the
  environment, as a smoke check that a real sshd changes nothing.
- Protocol: no daemon wire change, so `docs/protocol.schema.json` is
  untouched; the whisper envelope gets its own short spec section in the
  protocol docs, same additive-forever rules.

## Decisions to record as ADRs at implementation

- Events ride the terminal stream in-band; knock-once before whispering
  (the passthrough promise is bent by exactly one discardable sequence).
- The remote agent's lifetime is the ssh session's; persistence is
  explicitly another layer.
- macOS is a headless target; the desktop is Omarchy's.
