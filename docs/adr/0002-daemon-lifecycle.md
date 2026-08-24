# Daemon lingers, upgrades kill the old daemon, registry is connection-scoped

`amond` (the `amon daemon` subcommand of the single `amon` binary) is
auto-started by every amon command and then lingers until explicitly killed —
no refcounting, no idle shutdown. A newer amon version finding an older daemon
shuts it down and takes over. The registry is purely a reflection of live
wrapper connections: an agent disappears the instant its wrapper's connection
drops, and nothing is persisted across daemon restarts.

This works because the wrapper treats its daemon link as strictly best-effort:
it must never crash or disturb the wrapped agent regardless of socket state,
and it reconnects with exponential backoff (reusing the startup
connect-or-spawn primitive), re-registering on every reconnect. A daemon
takeover therefore just causes a brief re-registration storm, after which the
new daemon has rebuilt the registry from live connections.

## Considered Options

Refcounted daemon exit (quit when last client disconnects) was rejected in
favor of linger-forever + kill-on-upgrade. No-reconnect variants were rejected
because any daemon death would permanently orphan running agents.

## Consequences

The wire protocol must stay backwards compatible forever (additive-only
changes, tolerant readers), since old wrappers may talk to newer daemons.

## Postscript: one command deliberately does not start it

"Auto-started by every amon command" gained an exception. `amon focus`
(ADR-0011) never starts the daemon: it sits between a keypress and the screen
changing, so it asks a running daemon within a hard 150ms bound and otherwise
degrades to the plain workspace switch — a keybinding must not pay for a
daemon launch. Everything else still starts the daemon on demand, and the bar
widget (ADR-0008) never did: the bar observes, it does not summon.
