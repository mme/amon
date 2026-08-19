# The config file is TOML, and the daemon is the only thing that reads it

Amon has had no configuration at all. Adding one is not just a parser: two
runtimes need the answers. Rust plays the sounds, and QML draws the bar, and a
file both read directly is a file they can disagree about.

## TOML, served to the widget over the socket it already has

QML parses JSON natively and cannot parse TOML. That is the whole argument for
JSON, and it loses to a smaller one: **JSON has no comments**, which is
disqualifying for a file whose entire purpose is to be opened and edited by
hand. It would also mean two readers and two file-watchers that can fall out of
step — the bar showing glyphs from one revision while the daemon rings bells
from another.

So the file is TOML, and the **daemon is the only thing that reads it**. The
widget already holds a live subscription for agent state; configuration arrives
on that same connection. One parser, one watcher, one source of truth, and the
widget never learns where the file lives.

Live reload falls out of that rather than being built: the daemon watches the
file, re-reads on change, and broadcasts. QML property bindings apply the new
values on the next frame, so glyphs and timings change under a running bar with
nothing restarted. Sounds pick up the change at the next transition.

TOML is also amon's existing idiom — the detection manifests are TOML — so it
is one format in the repository rather than two.

```toml
# ~/.config/amon/config.toml
[bar]
frame_ms     = 200          # one spinner frame
state_beats  = 5            # beats showing the agent's state
marker_beats = 3            # beats showing the focus marker

[bar.glyphs]
working = ["⠒", "⠰", "⠤", "⠆"]
blocked = "󰋗"
done    = "󰗠"
focused = "󱓻"

[sound]
enabled = true
# done    = "~/sounds/ding.mp3"
# blocked = "~/sounds/alert.mp3"
```

Glyphs are plain characters, deliberately. Not markup, not a styling language:
the idle workspace keeps the underline it computes for itself, and nothing in
this file can change how a glyph is drawn — only which glyph it is. A config
that could set arbitrary rich text would make the bar's appearance amon's
problem to support rather than its own to decide.

## The daemon plays the sounds

Not the wrappers. The daemon sees every transition, already holds the registry,
and will already hold the config; wrappers would mean one config parser per
agent and several processes racing to play the same ding at once.

Two sounds, matching herdr's, on the transitions amon already names:

- **`blocked`** — the agent needs a human. Plays on every entry to `Blocked`.
- **`done`** — the agent finished. Plays on entry to `Idle` **only when the
  result is unseen** — that is, only when the agent finished while you were not
  looking at it.

That second rule is a deliberate divergence from herdr, which rings on every
transition to idle. Amon knows more than herdr does here: Seen is exactly the
fact that separates "it finished" from "it finished and you watched it happen"
(ADR-0007), and a notification for something you are already looking at is
noise. It is the same reasoning that puts Done above Working in
`AgentEntry::attention` (ADR-0011).

**A crossing has to settle before it is worth a noise.** Detection passes
through intermediate states on its way to a settled one: a new prompt reads as
idle for a moment before the rule that recognises it as a question matches.
Playing immediately therefore turned one event into two — a finish, then a
request. So a crossing is recorded rather than played, and plays a second later
only if the agent has not moved on; anything that happens in between cancels it,
including a change worth no sound at all, because that is still evidence the
earlier one had not settled. herdr does the same thing with the same default
second, which is why its immediate path is the one behind a config flag rather
than the normal one.

Playback follows herdr's approach — spawn the first available of `paplay`,
`pw-play`, `ffplay`, `mpg123`, `mpv` and give up silently when none exists. A
missing audio player is not an error to report; it is a machine that does not
do sound. Only those five, and so only Linux: amon's one target platform is
Omarchy, and herdr's macOS and Windows branches would be code carried for
nobody. A different platform reads as "no player available" and stays quiet.

## Consequences

**The bundled sounds sit outside the vendoring script.** `scripts/revendor.sh`
copies upstream files through `vendor_file`, which prepends a provenance header
and runs the token map over the contents; both would corrupt an mp3. Rather
than grow a third category in the script for two files that will never change,
they are copied in by hand once, and NOTICE carries the attribution a comment
header normally would. The cost is that they are the one vendored thing
`just revendor` cannot refresh — recorded here because ADR-0005's rule is that
vendored code is re-derived mechanically, and this is a stated exception to it
rather than an oversight.

**The file is polled, not watched.** A second of latency on a file that is
edited by hand a few times in its life is not worth a dependency: polling its
mtime is a dozen lines in the background thread the daemon already runs for
manifest refreshes, and `notify` would be a new crate in the tree to save that
second. Statting the path each time rather than holding a descriptor also means
an editor that saves by rename — which is most of them — is picked up like any
other change, and a deleted file reads as "back to defaults" for free.

**The widget's appearance now depends on a daemon.** With no daemon it already
falls back to plain workspace numbers, so it will fall back to built-in default
glyphs by the same path — the defaults live in the QML, and configuration only
ever overrides them. A bar that cannot reach amon looks like a bar without amon,
which is the property that already held.

**A broken config must never break the bar.** Unparseable TOML keeps the last
good configuration, and an empty or missing file means defaults. The error
surfaces in `amon doctor`, which is where the other "installed but wrong"
conditions already surface. Refusing to start, or falling back silently with no
way to notice, are both worse than running on yesterday's answers and saying so.

**The protocol grows.** Configuration becomes a request and an event, so
`docs/protocol.schema.json` moves and a widget built against an older daemon
gets no configuration rather than a broken connection — the same
forward-compatibility the registry's partial updates already assume.

**Per-agent sound overrides are not carried over.** herdr's config can turn
sounds on or off per agent kind. Nothing has asked for that here, and the
smaller surface is easier to widen later than to narrow.
