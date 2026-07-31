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

## Consequences

The input path is no longer a pure pass-through: it must parse well enough to
recognize and conditionally swallow `CSI I`/`CSI O`, and the swallow decision
depends on mode state owned by the output path (the Shadow Terminal). Focus
correctness inside multiplexers depends on the multiplexer forwarding focus
events per-pane (tmux: only with `focus-events on`; herdr: to be verified).
If terminal state restoration is ever skipped (crash), a terminal is left
with 1004 enabled and the shell may print stray `^[[I`/`^[[O` — annoying but
harmless, and re-entering any 1004-aware program clears it.
