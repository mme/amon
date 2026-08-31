# The row says what the agent is doing, in the harness's own words

ADR-0017 removed a column that said what an agent was doing and recorded why:
the source was Claude's terminal title, which is written once from the opening
exchange and never revised, so the column went on confidently naming the first
thing that had been asked. It closed by saying the field would look like the
answer again, and that anything replacing it would need evidence.

This is that column, from a different source. `AgentEntry::activity` carries
the line the harness itself last put on screen to narrate its own step —
`Reading 1 file…` while it works, the opening line of the reply when it is
done — read off the rendered terminal the wrapper already keeps.

## Why this source is not the last one

The title failed because it was *frozen*: stable not because it was tracking
anything but because nothing rewrote it. A marker line is the opposite. It is
rewritten at every step of every turn, which is measurable rather than
asserted — the captured sessions in `crates/amon-term/tests/fixtures/` replay
to four and three distinct lines respectively, each one describing the step it
was drawn for.

It is also not amon's phrasing. A column reading `Working…` would repeat the
glyph and the state beside it and add nothing; the harness has already written
a better line for the human sitting in front of it, and relaying that is the
whole value. Where harnesses disagree — Claude narrates a tool call in the
present tense and a reply in the past — the disagreement is theirs and travels
with the text.

## Why the screen, and not hooks or transcripts

Hooks look like the obvious source and are not. Of the vendored integrations,
only `pi` and `omp` send a `message` at all, and both populate it solely for
`blocked` — the question that blocked, or a retryable provider error. Working
and idle send nothing. Claude and Codex send no message field. Measured
against the alternative, the per-agent surface is about 5,900 lines of hook
assets and installers versus 1,600 lines of declarative detection data, for a
signal that does not carry what this column needs.

Transcripts would work and are per-agent file formats amon does not otherwise
read, needing a reader per harness and a session id to find the file with. The
screen is already rendered, already correct, and already the thing the user is
looking at.

The blocked-only `message` those two hooks do send is separate and still
dropped: `ReportState` has no field for it even though the vendored
`set_hook_authority_at` takes one and is handed `None`. Worth fixing, not here.

## Why the carriers are amon's file and not manifest rules

Which line to read is data — a region of the screen and a pattern that
recognises the line in it — and its natural home looks like the agent's
detection manifest. Two things rule that out.

The engine matches but does not capture: every matcher ends in `is_match`, and
the only text it reports back is a 240-character head-anchored preview of the
region. The line worth reading sits at the tail of a region deep enough to
clear the input box, past 240 characters at any real terminal width.

The larger reason is that a manifest is not amon's to extend. The daemon
fetches manifest updates and they land per agent, so a rule patched into a
bundled file is dropped by the next update to that agent, and a local override
carrying it shadows every future update instead
(`local_override_shadowing_remote`). Either way the feature and upstream's
detection improvements become each other's enemies. `carriers.toml` keeps
amon's data in amon's own file and leaves the manifests entirely upstream's,
which is what ADR-0001 vendored them for.

What is emphatically not duplicated is logic. The region is evaluated by
herdr's own accessor, so where a harness draws its input box stays vendored
knowledge that keeps tracking upstream across re-vendors. Whether a screen can
be believed at all is herdr's verdict, taken from the same
`skip_state_update` that already suppresses state — including its quirk that
only the winning rule contributes the flag. Adding an agent is adding an
entry; there is no per-agent code.

## Consequences

**`vendor/patches/0005` widens `manifest::region` to `pub`.** One word, the
same category as `0002`, and the alternative was re-implementing region
selection — which is precisely the drift ADR-0001 exists to prevent. A
re-vendor that moves the function fails loudly at `git apply`.

**A screen that cannot be believed holds rather than clears.** The marker
blinks while a tool call is in flight, a viewer takes the terminal, a redraw
gets caught half-finished. On all of those the honest answer is what the agent
was doing a moment ago. Only a session ending — a `/clear`, a `--resume`, a
new session id — drops the line, which nothing on screen marks and only the
wrapper knows.

**A torn frame is relayed as drawn.** Feeding partial writes can render a line
mid-rewrite, and a capture shows `Searing 1 directory…` between two correct
frames. That is what the user's own terminal displayed at that instant, so
reproducing it is right; smoothing it would mean amon disagreeing with the
screen.

**The elastic column is back, bounded.** ADR-0017 left every column
fixed-width. Activity has no natural length, so it is sized to a budget rather
than to the longest line present: a verbose harness elides its own tail
instead of pushing the columns that identify a row off the pane. It is
rightmost and the first column dropped when the pane narrows — the
elaboration on a row, never what identifies one.

**Claude only, deliberately.** `carriers.toml` has one entry. Codex, pi and
omp read nothing rather than guess, since their screens are characterised but
their carriers are not written. The captures under
`crates/amon-term/tests/fixtures/` and `examples/harness-probe.rs` are how the
next one gets added, and a replayed capture is how it stays added.
