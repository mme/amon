# No column says what an agent is doing

Every column in a row says *where* an agent is: the Project, the branch, the
directory. None says what it is doing. The terminal title looked like the
answer — it was already arriving on the wire and the panel was simply dropping
it — so it was built, shipped, and then removed. This records why, because the
field is still sitting there in every agent's terminal, and it will look like
the answer again.

## What was built

The wire carried a **Title**: the OSC 0/2 terminal title with its leading
activity glyph stripped, suppressed when it merely repeated the Project, the
directory or the Agent Kind's own name. The wrapper computed it, so `amon
status` and the panel could not disagree; agents adopted from herdr got the
same treatment where they were adopted, since they have no wrapper.

It worked. The column rendered, the churn was gone, codex's echo of the project
name was suppressed. And it was useless.

## Why it came out

Claude's terminal title is its session title with a spinner glyph on the front,
and Claude Code writes that title **once, from the opening exchange, and never
revises it**. Measured on a real session: 62 writes of the `ai-title` record,
one distinct value. The session had long since moved from the question it
opened with to something unrelated, and the column went on confidently naming
the first thing that had been asked.

That is worse than an empty column. An empty column costs a glance; a column
that is confidently wrong costs a glance *and* a wrong belief, and there is
nothing in the row to tell you which one you are looking at. The whole point of
the panel is triage — deciding which agent to go to — and a stale label is
exactly the input that makes that decision badly.

The alternative source is the transcript's `last-prompt`, which does track the
work. It is 201 characters of raw prose rather than a label, it needs the
transcript reader that has not been built, and it is Claude-only. Reaching for
it would mean building a new extraction path to rescue a column that had
already failed once on its own merits. Not now, and not without evidence that a
row needs more than the Project and the branch to be told apart.

## Consequences

**`AgentEntry` has no `title`.** It predates this and was unused, so nothing is
lost — but leaving it would have cost something. Carried raw it churns: Claude
rewrites the title ten times a second to turn its spinner, so every frame is an
update to a field nobody reads. Carried stripped it keeps a whole apparatus
alive with no consumer. Neither earns its place.

**The panel's columns are all fixed-width now.** The Title was the elastic
column — the one that absorbed slack and gave it back before anything had to be
dropped. Without it the layout is simpler: natural widths, and columns drop
right to left when they do not fit.

**Detection is untouched.** It never used the wire field. Manifest rules match
against the raw terminal title read straight from the shadow terminal, spinner
glyph included — that glyph is what several of the rules key on.

**herdr's `title` is no longer deserialized.** The projection ignores it like
every other field of herdr's record amon does not use.

**The lesson generalises past this field.** "The agent already emits it" is not
evidence that it is worth showing. What matters is whether it stays true, and a
value can be perfectly stable because it is frozen rather than because it is
tracking anything — which is how this one read as a good source right up until
it was on screen next to work it no longer described.

## Superseded in part

ADR-0020 adds the column back from a different source: the line the harness
draws to narrate its own step, read off the rendered screen. The reasoning
here stands — it is why that one had to arrive with evidence that its source
tracks the work rather than freezing.
