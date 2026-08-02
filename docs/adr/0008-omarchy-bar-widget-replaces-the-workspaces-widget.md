# The Omarchy bar widget replaces Omarchy's own workspaces widget

Omarchy Quattro's bar is [Quickshell](https://quickshell.org/) — one
long-running instance (`quickshell -n -p /usr/share/omarchy/shell`) hosting
everything as plugins, third-party ones loading from
`~/.config/omarchy/plugins/<id>/` behind a `manifest.json`. Two facts about it
decide this ADR:

- A bar layout is a flat list of widget ids in `~/.config/omarchy/shell.json`.
  There is no decoration or overlay mechanism — a plugin cannot add anything
  *on top of* another plugin's widget, and `Bar.qml` is Omarchy-owned code
  users are told not to edit.
- Quickshell speaks unix sockets natively, so a widget can be a Subscriber
  directly: `hello`, `subscribe`, then live events off `amond.sock`. No bridge
  process, no polling `amon status`, no shelling out.

What we want in the bar is one dot **above each workspace number**, coloured
by the most urgent agent state in that workspace. That placement is the whole
point — it answers "where", not merely "whether" — and it is only reachable
from inside the widget that draws the workspace numbers.

So amon ships `amon.workspaces`: a copy of Omarchy's `Workspaces.qml` with a
dot layer added, which the user swaps in for `omarchy.workspaces` in their bar
layout.

The alternatives, and why not:

- **Wrap the upstream widget** — instantiate Omarchy's `Workspaces`
  unmodified and paint over it. No forked code, but positioning a dot over
  each workspace button needs those buttons' geometry, which the widget does
  not expose; it would mean walking its child items at runtime and breaking
  silently whenever upstream restructures its layout. Brittle in a way users
  cannot diagnose.
- **Upstream a per-workspace badge hook into Omarchy** — the right long-term
  answer, and worth pursuing, but it does not exist and would put amon's
  release cadence inside someone else's.
- **A separate widget beside the workspaces** — no fork at all, but it gives
  up the placement that motivated the feature.

## What the widget shows

Deliberately narrow, because a bar is a glance and not a report:

- One dot per workspace that has agents, showing that workspace's most urgent
  state. A workspace with no agents shows nothing.
- Working blue, done-but-unseen green, blocked amber, idle-and-seen a hollow
  circle. `Unknown` shows nothing — it is usually a non-agent process. Red is
  reserved for an error state amon does not have; the four states stay
  herdr's, and the bar invents nothing the daemon cannot observe.
- Agents whose workspace could not be resolved — tmux, ssh, an unmapped
  window — are absent from the bar. `amon status` remains the complete view.
- No daemon means no dots and a quiet retry. Dots are cleared on disconnect
  rather than left stale: an indicator that can lie is worse than a blank one.
  The widget never starts the daemon; the bar observes, it does not summon.
- Clicking a workspace does what it did before — switch to it. Seen-ness then
  updates itself, because arriving focuses the agent's terminal, which reports
  focus, which flips `seen` (ADR-0007), which the subscription pushes straight
  back into the bar. The dot clears because you looked at it.

## Consequences

The fork carries the cost forks carry: Omarchy improving its workspaces widget
does not reach anyone using amon's until we re-sync. The repo already has the
discipline for this — ADR-0005 forbids hand-editing vendored code and
re-derives it mechanically — and the same rule applies here, with one
addition: **the behavioural diff from upstream stays at zero.** The only
difference is that a dot appears. Every re-sync is then a mechanical
re-application of one layer, not a merge of two designs. That is also why the
click behaviour was left alone.

The widget is a protocol client, so it must not drift from the daemon it
parses. It therefore ships inside amon and installs with
`amon install omarchy`, which puts `amon integrations` in charge of reporting
whether the installed copy is current — the same machinery that already covers
agent hooks. Installing writes the plugin directory and then prints the
`omarchy plugin enable` and `omarchy bar plugin add` lines rather than editing
`shell.json`, because that file becomes canonical the moment a user touches it
and Omarchy does not deep-merge.
