# A terminal is what makes an agent worth tracking

An agent that starts another agent gets it a row. Claude Code running `codex`
in the background reaches amon through the same door a person would — the
shell it runs commands through has `expand_aliases` set, and
`alias codex='amon codex'` is in the rc that shell sources — so amon wraps it,
registers it, and the Agent Panel grows a line for something with no window and
no cursor. `amon focus` cannot land on it. ADR-0013 already named this shape
when rejecting a different mechanism: *a row with nowhere to jump to*.

So amon runs an agent bare — no PTY, no observer, no daemon link, no row — when
it has a terminal on neither side.

## A terminal on neither side is the test

Measured inside a live Claude Code, on the subprocess it runs commands in:

```
fd0       /dev/null                     not a terminal
fd1       …/tasks/….output              not a terminal
/dev/tty  No such device or address     no controlling terminal at all
```

It puts those subprocesses in a session of their own, so there is not even an
inherited terminal to mistake for one. This is not a heuristic that happens to
fit; there is nothing terminal-shaped within reach of such an agent, because
nothing about it is meant to be looked at.

## Why not ask who started it

The obvious alternative reads `AMON_AGENT_ID` out of the environment. An agent
started by a wrapped agent inherits it, environments descend through shells and
`setsid` and double-forks alike, and the Shim already trusts exactly this marker
to mean "amon is already here" (ADR-0013). It was measured too, and it is
present as expected.

What decides against it is that it answers a narrower question than the one
being asked. A cron job, a CI step, a systemd unit and an editor plugin all
start agents with no amon anywhere in their ancestry, and every one of them is
as un-jumpable as the subprocess case. Ancestry catches one source of
untrackable agents; a terminal is the property they all share, and the property
the panel is actually about, so it is worth asking about directly rather than
through a proxy that correlates with it.

The environment does not stop mattering, though; it moves to the way out. See
the first consequence.

## Both descriptors, and not `/dev/tty`

Either side is enough to wrap. `amon claude > log` keeps a terminal on stdin,
`echo prompt | amon claude` keeps one on stdout, and in both a person is sitting
there. Only when neither is a terminal is there nobody to show a row to. Testing
stdout alone would take the row away from someone redirecting their own agent's
output while watching it work.

`/dev/tty` is the wrong instrument even though it would have worked here. A
controlling terminal is inherited by everything a terminal-borne process starts,
so it answers "was anything in my ancestry started from a terminal" — which
stays true for a background agent whose parent did not `setsid`, and would make
the check depend on an implementation detail of the agent that spawned it.

## Consequences

**An outer wrapper's routing must not follow the agent in.** A bare agent that
still has `AMON_ENV`, `AMON_AGENT_ID` and `AMON_SOCKET_PATH` in its environment
reports its state to a wrapper watching something else entirely — one agent's
hooks driving another agent's row. The step-aside path scrubs all three. This is
the same scrub the herdr bypass has always needed for the same reason, which is
why both go through one `exec_bare`.

**This is a policy, not a special case for subprocess agents.** A headless
`amon claude -p …` in your own script loses its row too. That follows from what
the panel is for — a row is an offer to go somewhere — but it is a wider rule
than the case that prompted it, and it should be stated as one.

**It rests on the outer agent handing over pipes rather than a PTY.** A harness
that allocates a PTY to coax colour out of a subprocess would look interactive
here and get its row back. That costs the suppression, not correctness: amon
behaves exactly as it did before this check existed. The assumption is written
down beside the check rather than left to be rediscovered.

**Tests have to hand agents a terminal.** `Sandbox::spawn_agent` piped all three
descriptors, which is now the bare path — thirteen tests would have had their
agent step aside, and the ones waiting for it to register would not have failed
loudly so much as stopped having a subject. It opens a pty for stdin instead.
That the test harness had to change is the honest signal that this is a
behaviour change and not a bug fix.

**The Shim decides before the wrapper ever sees the agent, and it decides
differently.** An agent reached through the PATH shim rather than through the
alias never arrives here at all: the shim `exec`s the real agent the moment it
sees `AMON_AGENT_ID` (ADR-0013). That is right for the case it was written for
— amon spawning its own child, which must not be wrapped twice — and it cannot
tell that case apart from one where the variable merely came down from an
ancestor. So an agent that a wrapped agent starts *without* a shell keeps the
outer wrapper's `AMON_ENV`, `AMON_AGENT_ID` and `AMON_SOCKET_PATH`, and its
hooks can still report into the outer agent's row.

Nothing here makes that worse and nothing here fixes it. Telling "amon is my
parent" from "amon is somewhere above me" needs a mechanism the shim does not
have — the variable alone cannot carry the distinction — and that is its own
change. Worth knowing that the alias door and the shim door do not currently
answer this question the same way.
