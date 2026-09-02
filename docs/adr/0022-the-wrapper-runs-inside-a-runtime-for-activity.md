# The wrapper runs inside a runtime, for activity, wearing the agent's name

ADR-0016 has the wrapper step aside inside a runtime pane (`HERDR_ENV`,
`LUVUS_ENV`): it `exec`s the agent and wraps nothing, because the runtime is
the detection authority there and a second wrapper would only fight it. That
was right when the wrapper's whole job was reporting state. It is wrong now
that the wrapper is also the source of *activity* — the prompt and the
narration — and will later be the seat for *control* (sending a command, a
stop). Neither of those can come from a process that has `exec`d itself away.

So inside a runtime the wrapper no longer steps aside. It wraps the agent the
way it wraps everywhere else — its own pseudo-terminal, the agent as its child,
bytes passed through untouched — and changes only *what it reports*.

## What it reports: activity, not state

The runtime keeps everything it already owns: the agent's state, its window,
its identity, the row you jump to. The wrapper, inside a runtime:

- **registers no row.** The runtime's adopted entry is the only one.
- **reports no state.** The runtime detects that from the screen it owns.
- **reports only activity** — the prompt and narration it already collects from
  hooks and the shadow terminal — onto the runtime's existing row.

One row, two contributors: the runtime says where the agent is and how it is;
the wrapper says what it is doing (and, later, makes it do things). This is the
same split ADR-0020 drew between state and activity, now drawn across two
processes instead of one.

## The problem: the runtime identifies the agent by its name

A runtime pane scans its process to decide which agent it is looking at — and
crucially, by the process's *name*, never its executable path. Verified in both
upstreams at the vendored/pinned commit:

- **herdr** reads the foreground process's **comm** (its short name; on Linux
  `argv0` is never populated, so comm is the whole of it) and matches it against
  known agents, trying to see *through* a fixed list of generic wrappers —
  shells, `node`, `python`, `tmux` — but nothing else. It never reads
  `/proc/<pid>/exe`; the only path it reads is the cwd.
- **luvus** reads the pane subtree's **command lines** (argv, via `ps`) and
  matches the first token's basename per process, unwrapping interpreters. It
  reads `/proc/<pid>/exe` only to find *its own* server to stop, never to name
  an agent.

When the wrapper interposes, the pane's foreground process is `amon`, and the
real agent runs in the wrapper's own pseudo-terminal, off the pane's foreground
group. So herdr sees `amon` and shows nothing. (luvus, which walks the whole
subtree, can find the real child — but must not be relied on to.)

## The fix: the wrapper wears the agent's name

Because neither runtime reads the executable path, the wrapper can present the
agent's name and be believed. Inside a runtime it sets both fields the two
runtimes read, to the agent's own command name:

- its **comm**, via `prctl(PR_SET_NAME)` — what herdr reads.
- its **process title** (argv0 / `/proc/<pid>/cmdline`), via an in-place
  rewrite of the argv region — what luvus reads.

It is not a guess. The wrapper carries herdr's own identification code
(vendored), so it sets exactly the string the runtime will match, and it is the
process launching the agent, so it knows which string that is. An agent name
fits comm's 15-byte cap in every case we ship.

This is a deliberate, named divergence, and its one cost is a tie to the
runtimes' scan: were either to start reading the executable path, the name
would stop being believed. That break is loud — it surfaces the moment a
re-vendor's tests or a live check show no agent — not silent, and amon already
tracks both upstreams closely. The wrapper wears the name **only inside a
runtime**; everywhere else it is plainly `amon`.

## The join: activity finds the runtime's row by pane

The wrapper's activity and the runtime's row are made by different parties with
different ids, so the daemon joins them. The key is the **pane**: both runtimes
inject their pane id into the pane's environment (`HERDR_PANE_ID`,
`LUVUS_PANE_ID`), and the adopted entry is already stamped with that same pane
id (herdr's `pane_id`, luvus's `pane`). So the wrapper tags its activity with
the pane it is in, and the daemon's runtime supervisor — which already tracks
which entry is in which pane and refreshes it every snapshot — applies the
activity to the matching adopted entry.

The pane is not the sturdiest possible key: a terminal can move to a new pane,
and the wrapper's environment stamp is fixed at launch. The sturdier key is the
agent's own **session id**, which does not move — but herdr's snapshot does not
expose it (its agent record carries terminal id, pane, cwd, status, not
session), so it is not available to key on today. Pane it is, with `agent` +
`cwd` as a fallback match, and session id noted as the upgrade if a runtime ever
surfaces it. A stale pane match degrades to no activity on that row for a
moment, never to activity on the *wrong* row: the fallback requires the agent
and cwd to agree too.

## Consequences

**Coverage inside a runtime is the hook-and-screen split of ADR-0020, minus
nothing amon can read.** The wrapper still owns a shadow terminal, so screen
carriers work (claude's `●`, codex's bullets, grok's `◈`), and the hook socket
is live, so prompt and narration hooks work. The full activity picture is
available inside a runtime exactly as outside it — the runtime just owns the
state and the row.

**The step-aside for a non-terminal context stays.** ADR-0016's other reason to
step aside — no terminal on either side, so no window to jump to — is untouched.
This ADR narrows only the *runtime-pane* step-aside, and only to "wrap, but
report activity not state, wearing the agent's name."

**Control is the reason, not a side effect.** The wrapper is kept alive inside a
runtime so that a future "send this command" or "stop" has a process in the I/O
path to do it. That it also carries activity today is what makes keeping it
alive worth its weight now.
