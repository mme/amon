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

## The Turn, and the prompt

The first cut answered one question — what is the newest narration line? — and
so shipped a bug it is worth naming: the reader took the newest `●` anywhere in
the region, so a freshly submitted prompt showed *the last thing the agent said
about the previous one* until the new turn narrated. That is ADR-0017's
confidently-wrong failure, reintroduced.

The fix is a second question underneath: which **Turn** are we in? The
submitted prompt is the boundary. A narration counts only when it sits after
the newest prompt on screen; one before it belongs to a finished turn. A turn
with no narration yet shows its **Prompt** — which is also the honest answer in
the gap between submitting and the first narrated step, where the row used to
say nothing. The two kinds rank kind-first: narration beats prompt whenever
both exist, and within a kind a hook's exact text beats the screen's rendering
of it.

The prompt regions structurally exclude the live input line — Claude's
`above_prompt_box` ends at the box border, Codex's `before_current_prompt_marker`
is herdr's own cut between the live prompt and earlier ones — so text still
being typed belongs to no turn and appears nowhere. Two prompt-shaped lines
that are not asks are rejected in data: Claude's startup `Try "…"` tip and a
slash command, which would open a turn no work closes.

Where a harness offers a prompt hook, that is the better source than the
screen: exact text rather than a wrapped rendering, an event rather than an
inferred boundary, and no startup-tip or slash-command noise to reject. Claude
has one — `UserPromptSubmit`, which herdr registers only to *remove*, having
used it for state before screen detection replaced it. amon installs its own
script for it beside the vendored hook (ADR-0021: a seam, editing `settings.json` through herdr's own
formatting-preserving CST splice — only the bytes that change are touched,
never a whole-file reserialize; `vendor/patches/0006` and `0007` widen the
primitives a seam needs, which is a mechanical visibility change and so a
patch), runs after herdr's install since that strips the event, and owns the
script's removal and doctor line. The screen prompt stays the fallback for
every harness without such a hook. A new wire method, `agent.report_prompt`,
carries it — amon-only, since herdr collects state, not content.

`AgentEntry::activity` is now `{ text, kind }` rather than a bare string,
because the panel styles a prompt differently from a narration — a `❯` marker
and italic — and a string cannot carry which it is. Nothing had shipped, so it
is a clean break rather than a compatible one.

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

**The elastic column is back, and it is the one that gives way.** ADR-0017
left every column fixed-width, so a narrow pane could only drop columns whole.
The message has no natural length and takes whatever the fixed columns leave,
which turns that around: slack now has somewhere to go, and a pane that
narrows costs the message its tail rather than costing a row something that
identifies it. Only when the remainder is more ellipsis than words does the
column come out, and the branch is the only thing droppable behind it.

**The kind and the state columns came out to pay for it.** Both were words for
something already on the row — the glyph says the state, and a machine running
one kind of agent wrote the same name down the whole list. The age moves to
the far edge, where short right-aligned values read down as one column.

**Proof that the prompt is on screen is per-agent data, not code.** The first
version hardcoded herdr's `prompt_box_body`, which was Claude's box passing
for every agent's. Codex draws no box: its prompt is a bare `›` line, and
herdr's box finder latches onto the two rules Codex puts around an *answer*
and reports that as the input box — right on 2 frames of a 47-frame session
and wrong in a more misleading way on the rest, since the "box" it finds holds
the reply. The marker line is present on 38 of those 47. So an entry names
where to look and what to look for, defaulting to the box when a harness draws
one.

**A harness that marks its steps and its notices alike needs the difference
spelled out.** Codex bullets `• Explored` and `• Working (7s · esc to
interrupt)` identically, and the second is both the generic label this column
exists to avoid and a value that changes every second. An entry may carry a
`reject` pattern; scanning continues past a rejected line to the step behind
it. This is herdr's own `not` gate in a smaller form, and it is per-agent
wording, so it belongs in the data.

**Claude and Codex, deliberately not more.** pi is the reason to stop: its
prompts, tool calls and replies are identical as text and told apart purely by
background colour (`#343541`, `#283228`, unset), which no text region can
express — `detection_text()` discards attributes. Reading pi means either a
colour-aware region built on the emulator's cell attributes or its hook, which
today sends text only when blocked. Neither is an entry in this file. omp is
simply unmeasured. `examples/carrier-scout.rs` reports what a harness offers
before an entry is written for it, and a replayed capture is how it stays
written.
