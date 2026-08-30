# Runtimes live behind one seam

amon v0.2.0 learned to show agents living inside herdr sessions. luvus is a
second agent runtime of the same shape — a detached server owning the
terminals, a tty-owning client the user typed, a JSON-lines control socket,
the same five agent states — and its users deserve the same treatment. The
question was how to add it without ending up with two copies of "watch a
runtime's clients and project its agents", which is exactly the kind of code
that drifts.

## The decision

The daemon's herdr module became `runtime/`, and what it did splits in two:

- **What differs is a trait.** `Hosted` is the list of things that turned out
  to differ when the second runtime was written, and nothing more: how the
  runtime's processes are told apart (`classify`), where its socket is
  (`socket_for`), how to attach — gate on the protocol, subscribe, snapshot —
  and how a pushed event reads (`event`), plus the address the entry
  carries (`runtime`). Each runtime is a directory under `runtime/` that
  answers those questions and owns its wire quirks.
- **Everything else is generic.** Scanning `/proc` for attached clients,
  the per-session supervisor (handover between clients, the window follow,
  reconcile, the registry patches), and projecting a runtime's agent into an
  `AgentEntry` are one implementation, parameterised by `H: Hosted`. A bug
  fixed there is fixed for every runtime.

On the wire, `AgentEntry.herdr` became `AgentEntry.runtime`: an enum tagged
by `kind`, one variant per runtime. A consumer that only badges or filters
reads `kind`; the focus hop matches on the variant; a runtime that addresses
agents differently adds a variant with its own fields rather than bending
the others. An unknown `kind` fails to deserialize rather than being misread
as some other runtime's address. There is deliberately no free-form map:
fields that vary within a kind become an `Option` on that variant.

The wrapper's rule generalises with it: `amon <agent>` steps aside — execs
the agent bare with amon's routing scrubbed — when any runtime's pane env
var is set (`HERDR_ENV=1`, `LUVUS_ENV=1`). One detection authority per
context (ADR-0016); the list of those variables lives in `amon-protocol`
beside the other environment contracts.

## What the seam is not

Not a registry, not a plugin mechanism. Two implementations and a `match`
are the whole extensibility story, and a third runtime is a third directory
and a third variant. The cost of a registry would be paid on every read of
the code; the cost of a `match` is paid once, when the third runtime
arrives, and it is small.

## What the second runtime taught

Writing luvus against the trait is what shaped the trait. herdr subscribes
status per pane and needs a fresh subscription when the set of agent panes
changes; luvus has one global, sequenced stream and a cursor to resume from.
So the supervisor asks the runtime to `attach` given the panes it owns, and
a `STATUS_PER_PANE` flag says whether a changed set means re-attaching.
herdr has an "agent detected" event; luvus does not, and a pane's agent label
changing is that moment — so the event reader is given the labels amon
currently holds, and answers *structural* when they disagree. herdr's
terminal id outlives what runs in it and a counter tells restarts apart;
luvus's pane id is the identity and it has no counter — so the common
`HostedAgent` carries an `identity` and an optional `life`. None of this was
visible from herdr alone.

## Consequences

- The e2e suite now drives the real herdr and luvus binaries, pinned in
  `.mise.toml`, so a runtime release that changes the wire fails a test
  rather than a user. Bumping a pin is a reviewed diff whose CI run is the
  compatibility check.
- `AMON_HERDR=0` became `AMON_RUNTIMES=0`, and `AMON_RUNTIMES_ROOT=<dir>`
  limits adoption to sessions under a directory — what lets a test daemon on
  a developer's desktop drive a sandboxed session without adopting the
  developer's own.
- `amon focus` hops to the pane for any entry with a `runtime`, window or
  not. An agent the compositor never placed still gets the only half of the
  jump amon can make for it.
