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

What we want in the bar is the most urgent agent state of a workspace shown
**on that workspace's own indicator**. That placement is the whole point — it
answers "where", not merely "whether" — and it is only reachable from inside
the widget that draws the workspace indicators, for a reason worth writing
down because it is what closes off every alternative below:

> A workspace's label is not data the widget looks up. It is computed in one
> expression, `text: focused ? … : String(modelData)`, from a `required
> property int` fed by `Hyprland.workspaces`. The widget reads no settings, so
> there is no label map, no icon map, and nothing a `shell.json` entry or a
> second plugin could populate. Both ends of that expression are private to
> the file.

So amon ships `sh.amon.workspaces`: a copy of Omarchy's `Workspaces.qml` whose
label expression consults agent state, which the user swaps in for
`omarchy.workspaces` in their bar layout.

The alternatives, and why not:

- **Wrap the upstream widget** — instantiate Omarchy's `Workspaces` unmodified
  and paint over it. No forked code, but an overlay cannot replace a label,
  only cover it, and positioning anything per-button needs geometry the widget
  does not expose. Reaching the buttons at all means walking its child items at
  runtime.
- **Rebind the labels at runtime** — a plugin in the same QML engine could walk
  the object tree and reassign each button's `text` with `Qt.binding`. It
  installs no forked code, but it has to re-implement upstream's focus logic
  inside the new binding anyway, so the same lines are copied either way — just
  invisibly, and load-order-dependently. When upstream restructures, a fork
  fails loudly and this fails silently.
- **Upstream a per-workspace badge hook into Omarchy** — the right long-term
  answer, and worth pursuing, but it does not exist and would put amon's
  release cadence inside someone else's.
- **A separate widget beside the workspaces** — no fork at all, but it gives
  up the placement that motivated the feature.

## What the widget shows

Deliberately narrow, because a bar is a glance and not a report:

- A workspace's number is replaced by its most urgent agent state: working the
  braille spinner, blocked a circled question mark, done-but-unseen a circled
  check. Nothing is added beside or above the number, so the bar keeps exactly
  the width and height it had.
- Working animates with the same ten braille frames herdr shows in its own UI,
  at the same 80ms, driven by one timer for the whole bar so that two working
  workspaces step together. It is also what amon reads: ten of herdr's
  manifests decide an agent is working by finding a braille character in its
  title, so the bar is showing back its own evidence. Card suits were tried
  first and abandoned — a set only animates without twitching if every frame
  has the same ink box, and no heart in the font shares the other suits' grid.
- A workspace whose agents are all idle-and-seen shows its plain number, the
  same as a workspace with no agents at all. Nothing is wanted from you, so
  nothing is asked. Setting the two apart was tried, in italic, and dropped:
  the slant reads as noise rather than information, and upstream already draws
  the half of it that matters, dimming a workspace holding no windows to half
  opacity — and an agent's workspace always holds its terminal. `Unknown`
  counts as no agent: it is usually a non-agent process, and lighting the bar
  for it would cry wolf.
- The focused workspace keeps upstream's focus marker, which wins over agent
  state: which workspace you are on is something only this widget can say,
  while an agent's state is also one `amon status` away.
- Glyphs rather than colour, decided by building all three and looking at them
  on a real bar. A coloured dot row reads as noise at bar height, and glyphs
  small enough to sit above a number are indistinguishable from each other;
  taking the number's place gives the state the full label size. There is also
  no error state, because amon has none — the four states stay herdr's, and the
  bar invents nothing the daemon cannot observe.
- Agents whose workspace could not be resolved — tmux, ssh, an unmapped
  window — are absent from the bar. `amon status` remains the complete view.
- No daemon means plain workspace numbers and a quiet retry. State is cleared
  on disconnect rather than left stale: an indicator that can lie is worse than
  one that says nothing. The widget never starts the daemon; the bar observes,
  it does not summon.
- Clicking a workspace does what it did before — switch to it. Seen-ness then
  updates itself, because arriving focuses the agent's terminal, which reports
  focus, which flips `seen` (ADR-0007), which the subscription pushes straight
  back into the bar. The workspace goes back to its number because you looked
  at it.

## Consequences

The fork carries the cost forks carry: Omarchy improving its workspaces widget
does not reach anyone using amon's until we re-sync. The repo already has the
discipline for this — ADR-0005 forbids hand-editing vendored code and
re-derives it mechanically — and the same rule applies here, with one
addition: **the behavioural diff from upstream stays at zero.** Two lines are
replaced, `moduleName` and `text`, and one object is added. Every re-sync is
then a mechanical re-application of three marked changes, not a merge of two
designs. That is also why the click behaviour was left alone.

The copy is checked into amon rather than derived from the machine at install
time, which has a consequence worth stating plainly: **when Omarchy improves
its workspaces widget, installing amon's replaces that improvement with our
older copy, silently.** The drift tests compare the fork against a checked-in
fixture, so they measure our copy against the version we forked, not against
the version the user has. Deriving the widget at install — patching whatever
`Workspaces.qml` is on the machine and failing loudly when the anchors do not
match — would turn that silent regression into an error message, and remains
the obvious upgrade if the fork ever falls behind in a way anyone notices.

The widget is a protocol client, so it must not drift from the daemon it
parses. It therefore ships inside amon and installs with
`amon setup omarchy`, which puts `amon doctor` in charge of reporting
whether the installed copy is current — the same machinery that already covers
agent hooks. Installing writes the plugin directory without editing
`shell.json`, because that file becomes canonical the moment a user touches it
and Omarchy does not deep-merge; every `shell.json` change goes through
Omarchy's own CLI.

## Postscript: Omarchy 4.0 made the placement first-class

This ADR was written against a pre-release Quattro whose bar offered no
replacement mechanism, and its placement analysis is superseded. Released
Omarchy 4.0 honors an `omarchy.clonedFrom` manifest field for any plugin —
the machinery behind `omarchy plugin clone`, but read generically: enabling a
plugin that declares `clonedFrom: omarchy.workspaces` swaps it into the
built-in widget's own bar slot (same section, same index, per-entry settings
carried over), disabling swaps the built-in back, and IPC addressed to the
built-in id is routed to the enabled clone. amon's manifest declares exactly
that, so the three-command add/remove sequence this ADR described — and its
anchor-ordering caveat — collapsed to one `omarchy plugin enable`. The
pre-Quattro `omarchy bar plugin add/remove` commands no longer exist.

What is *not* superseded is the fork itself: a workspace's label is still
computed in one private expression from `Hyprland.workspaces`, so showing
agent state on the indicators still means shipping a forked `Workspaces.qml`.
`clonedFrom` changed only how the fork gets into the bar, not why it exists.
One caution for re-syncs: `clonedFrom` is honored generically in
`PluginRegistry.qml` and passes `omarchy plugin validate`, but the manual
documents it only in the clone-script section — semi-documented, so a future
Omarchy could narrow it; the enable/disable path is worth re-checking when
Omarchy majors.

## Postscript: the widget is no longer a `setup` target

The body above says the widget "installs with `amon setup omarchy`". That form
is gone. `amon setup <target>` names an agent and nothing else — putting a
desktop in the same slot meant one word covered two unlike operations: an agent
target installs a hook into a tool you run and aliases its name, a desktop
target installed a subscriber into your compositor and deliberately did not.
The two also differed in a way nobody could see from the command line, since
the target form installed the plugin without enabling it while the screen did
both.

Nothing else in this ADR changes: the widget still ships inside amon, still
installs into the plugin directory without editing `shell.json`, and is still
reported by `amon doctor` on the same machinery as the agent hooks. It now
arrives from the interactive screen or `amon setup --all`, both of which enable
it as well — so there is no longer a way to install it and leave the bar alone,
which is the one capability this removal costs.

The tests that covered install and removal moved with it, from the CLI's
end-to-end suite down to `crates/amon-integration/tests/desktop.rs`, where they
drive `desktop::install` and `desktop::uninstall` directly. They keep their own
`HOME` *and* their own `PATH`: uninstall shells out to `omarchy`, and a
developer's machine may ship a real one.

## Postscript: four marked changes, and the click is one of them

Two statements in the body are now false, and both were load-bearing.

**"Clicking a workspace does what it did before — switch to it,"** and the
consequence drawn from it, *"That is also why the click behaviour was left
alone."* It is not left alone. A click now runs `amon focus <id>`, the same
command Super+N runs (ADR-0011), so that the two ways of reaching a workspace
cannot land in different places. The fork is therefore **four** marked changes,
not three: `moduleName`, the `text` expression, the `AgentStates` object, and
`focusWorkspace`. The drift tests and the file's own re-derivation notes count
four; a re-sync re-applies four.

The zero-behavioural-diff rule the body sets out is what makes that a change
worth stating rather than smuggling. It was never quite zero — the label is the
feature — and the honest form of the rule is: the fork changes only what the
feature requires, each change is marked, and a test counts them. Where the
click lands is now part of the feature. The fallback keeps it a superset of
upstream's behaviour: `bar.run` is `bash -lc`, so the command is
`amon focus <id> || hyprctl dispatch …`, and a machine without amon on its
login-shell `PATH` switches workspace exactly as upstream does.

**The spinner is no longer herdr's ten braille frames at 80ms.** Those light
only dots 1-6, and a braille cell is four rows tall, so their ink fills the top
three quarters and the spinner visibly rides high above the digits beside it —
the bar centres the glyph's box, which does not centre its ink. It is now four
frames, `⠒⠰⠤⠆`, a two-dot bar rotating through the cell's two middle rows:
centred by construction, and light enough not to read as a blob at bar size.
Filling all four rows was tried first and was correctly centred but too heavy.

The cadence is re-derived rather than inherited: 200ms, so four frames turn
once every 800ms, which is what herdr's ten at 80ms do. Keeping herdr's
interval would have spun a quarter as many frames four times as fast.

What survives is the reason braille was chosen at all. Nine of herdr's
manifests decide an agent is working by finding *any* character from the block
in its title, so the bar still answers in the alphabet it listens to. What no
longer holds is the stronger claim: codex's manifest matches those ten
characters exactly, and the frames the bar draws are no longer among them.

## Postscript: a rescan does not reload a changed widget

`enable` asks the shell to rescan, and a rescan looks like it should be enough
— `reloadPlugins` unregisters every plugin widget, calls
`Qt.clearComponentCache()`, and scans again. It is not. Measured against a
shell answering IPC normally: rewriting both the entry component and the nested
import it pulls in, then rescanning, left the bar drawing the previous copy of
each. Only `omarchy restart shell` picked them up.

Left unhandled this makes every widget change ship stale — a user upgrading
amon keeps the old widget until they happen to restart their shell, while setup
reports `installed and enabled`. So `amon setup` restarts the shell, but only
when it replaced files under a running one: the widget's status *before*
install is read, and only `Outdated` qualifies. A first install has nothing
cached to be stale and an unchanged one rewrites the same bytes, so neither
disturbs the session. That case reports itself differently —
`updated and restarted` rather than `installed and enabled` — because a bar
blinking mid-setup should be accounted for rather than mysterious.

This is worth raising upstream, and if a rescan ever does pick up changed
plugin QML the restart deletes itself.

## Postscript: an idle agent is set apart after all

The body says a workspace whose agents are all idle-and-seen "shows its plain
number, the same as a workspace with no agents at all", and that setting the
two apart was tried in italic and dropped. They are set apart: an idle agent's
workspace shows its number underlined.

The reasoning in the body was that an agent at rest asks for nothing, so
nothing should be asked of the reader. That still holds for *urgency* — an idle
workspace must not compete with a blocked one — but it conflated urgency with
presence. "There is an agent here and it wants nothing" is worth knowing, and
is not the same fact as "there is nothing here"; collapsing them threw away the
only thing the bar could say about a resting agent.

Bold was the first attempt and could not be seen. Not for want of a bold face —
the family resolves to one that has it, so nothing was synthesized — but
because weight is a property of a glyph whose shape you already know, and a
lone digit on a bar has nothing beside it to be heavier than. An underline adds
a mark rather than thickening one, so it reads with no comparison available,
and it draws in descender space so the digit does not move.

Opacity was rejected on the same grounds the body uses to justify leaving
occupancy alone: upstream already dims a workspace holding no windows, so a
fainter number would place an idle agent between empty and occupied — quieter
in exactly the place more presence was wanted.

## Postscript: the focus marker lends its slot back

The body's rule was that focus wins outright — *"which workspace you are on is
something only this widget can say, while an agent's state is also one
`amon status` away."* The reasoning is sound and it is still most of the
policy, but taken absolutely it produced a hole: the workspace you are sitting
on is the one workspace whose agent the bar refuses to tell you about.

Usually that costs nothing, because you are there and can see the terminal.
Not always — the agent's terminal may be behind a full-screen browser, or on
the other end of a tiled row you are not looking at. In exactly that case the
bar is the only thing that could tell you, and it was the one place that
stayed silent.

So the focused workspace time-shares. When it holds an agent in any state other
than idle, it shows that state for 2000ms and its marker for 1200ms, and
repeats. The split is two named properties rather than numbers in a timer,
because those two numbers *are* the decision.

**The state gets the longer share, and that was decided by looking.** The first
version reasoned it the other way — the marker is what the widget alone can say,
so let it keep the large majority and let the state borrow just enough to be
noticed — and shipped 1:4. On a real bar it was wrong. Every ratio from 1:4 to
4:1 was tried, and the ones that read well all gave the state the longer half.
The argument that survives is the opposite of the one we started with: you
already know which workspace you are on, because the whole desktop is evidence
for it, so the marker only has to *confirm*. The agent's state is the thing you
cannot get any other way without leaving what you are doing.

The unit is half a turn of the spinner, not a whole one. A whole turn is the
tidier idea and was tried first — a state phase would be a whole number of
revolutions — but every split it can express was either too brief to read or
long enough to feel stuck, and the ratio that looked right falls on a half. It
is still derived from the cycle rather than written as a bare 400ms, so
changing the spinner's speed carries the blink with it.

An agent at rest borrows nothing. It asks for nothing, and against that the
marker is the more useful thing to be showing — the same reasoning that gives
a resting agent an underline rather than a glyph.

Two details worth keeping if this is ever re-derived. **The marker phase leads,
and restarts on arrival.** The state leading was tried first, reasoning that
arriving somewhere should tell you what is happening there; using it showed the
opposite. Moving between two workspaces that both hold busy agents never stops
the timer, so the new workspace inherited the old one's place in the cycle and
could land mid-state-phase — leaving a switch with no visual cue at all, since
both the workspace left and the one arrived at were showing agent glyphs. A full
marker phase from the first frame is the cue that the focus moved, and the state
is a second away regardless. And the spinner is *not* paused while the marker
shows — it is one tick for the whole bar, and stopping it would drift
this workspace out of step with every other working one.

The cost is that the marker is no longer constant, and it is now the smaller
share: on a workspace with a busy agent, "where am I" is answered for 1200ms
in every 3200. Fading between the two was tried and abandoned — the label is a
single `Text`, so the glyphs cannot cross-fade, and dipping the slot's opacity
to swap underneath left too little steady state at any rhythm worth having.
