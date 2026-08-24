# The Wrapper enables terminal focus reporting itself

Amon tracks Focus — whether the agent's terminal *view* has input focus — to
compute Seen ("has the user looked at this agent since its state last
changed"). The only signal with the right granularity is terminal focus
reporting (DEC private mode 1004): the compositor can say which *window* is
focused, but a window can host many views (tabs, herdr panes), and the
terminal is the only party that knows which view is showing. The Wrapper sits
on the PTY, so `CSI I`/`CSI O` focus events pass through it for free — but
only if mode 1004 is enabled, and agents cannot be relied on to enable it.

The trade-off was:

- **Passive**: observe focus events only when the agent itself enables 1004.
  Zero footprint, but yields nothing for agents that never enable it —
  `focused`/`seen` would silently not exist for exactly the agents amon
  exists to watch.
- **Hybrid**: passive, with compositor `activewindow` as fallback. No
  footprint, but Focus silently degrades to window granularity — wrong
  inside tabs and herdr panes, amon's own primary multiplexer story.
- **Active** (chosen): when the agent has not enabled 1004, the Wrapper
  enables it on the real terminal and *swallows* the resulting focus events
  from the input stream so the agent never sees bytes it didn't ask for. The
  Shadow Terminal tracks the agent's desired 1004 mode from the output
  stream; while the agent wants focus events they are forwarded untouched,
  and if the agent later disables the mode the Wrapper re-asserts it. On
  exit the Wrapper restores the terminal to the agent's last desired mode.

This deliberately spends a sliver of the "agent cannot tell amon is there"
invariant: amon still answers no queries and injects nothing into the
*agent-visible* stream, but it does hold terminal state (mode 1004) that the
agent didn't set. An agent could detect that only by querying mode 1004 via
DECRQM without having set it — no known agent does.

## What follows from it

Three things fall out of "the agent must not be able to tell", and each cost
something to get right:

- **Mode tracking belongs on the output path, not in the observer.** What it
  decides governs the agent's *input*, and the observer is best-effort by
  design: a shadow terminal that failed to start or fell behind would
  otherwise silently start stripping bytes the agent asked for. The wrapper
  therefore runs its own small mode scanner over the output stream, ahead of
  the write to the terminal, so the flags are already right when the terminal
  could first act on the bytes.
- **The scanner has to see disables the agent's own state does not show.** A
  `CSI ?1004l` from an agent that never enabled the mode still reaches the
  real terminal and still turns amon's reporting off, as does `ESC c`. Both
  are transitions, not states, which is why comparing the mode before and
  after a chunk is not enough.
- **Amon writes to the terminal only between the agent's sequences.** A chunk
  read from the pty can end mid-sequence, so a quiet gap proves nothing; the
  scanner's parser state does. One owner (`Screen`) serializes every write and
  restores the terminal on drop, which also covers the early returns and
  panics between enabling the mode and the agent's first byte.

Focus is *not* seeded from the compositor. That would be window granularity
answering a per-view question — the very thing this decision rejects — and a
wrong `seen` never corrects itself if no focus event ever follows. Instead the
view is assumed focused at launch, on the grounds that the user just typed the
command into it, and the first real report overrules that.

## Consequences

The input path is no longer a pure pass-through. It recognizes `CSI I`/`CSI O`
within a single read and never holds bytes back, which trades two rare
inaccuracies for never delaying a keystroke: a report split across two reads
reaches the agent, and a literal `CSI I` typed or pasted by the user is taken
for a focus report and swallowed. Holding a trailing `ESC` to disambiguate is
what gives multiplexers their notorious escape latency, and amon will not pay
that on every keypress.

Focus correctness inside multiplexers depends on the multiplexer forwarding
focus events per-pane (tmux: only with `focus-events on`; herdr: to be
verified). If restoration is ever skipped (SIGKILL), a terminal is left with
1004 enabled and the shell may print stray `^[[I`/`^[[O` — annoying but
harmless, and re-entering any 1004-aware program clears it.
