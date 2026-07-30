# Everything after the agent's name belongs to the agent

`amon <agent> [args...]` passes every token after the agent's name to the
agent verbatim. `amon claude --help` gives the agent its `--help`, not
amon's; `--json`, `--resume`, quoted arguments, empty arguments — none of it
is interpreted, reordered, or consumed. Amon's own flags exist only before or
instead of an agent name.

This is load-bearing, not cosmetic: users hand amon the exact command line
they would have run bare, and any token amon swallowed would change the
agent's behavior in ways the invisibility contract (README) forbids. The
invariant is guarded by the e2e test
`arguments_after_the_agent_belong_to_the_agent`.

## CLI implementation

The CLI is clap with `external_subcommand`: any first token that is not a
known amon subcommand becomes the agent, and clap hands over everything after
it uninterpreted. This replaced a hand-rolled parser whose stated reason was
this same invariant ("a parser that knew about flags would have to be told to
stop knowing") — clap can be told exactly that, and in exchange amon's own
subcommands get strict argument handling (`amon status --bogus` is an error,
not silence), generated help, and room for future flags.

## Consequences

Amon subcommand names (`status`, `install`, `uninstall`, `integrations`,
`daemon`, `hook`, `help`) shadow agents with those names; such an agent can
still be wrapped by path (`amon ./status`). Flags before an agent name are
amon's and error if unknown — `amon --resume claude` is an error rather than
an attempt to run an agent named `--resume`. The passthrough invariant now
rests on clap's external-subcommand behavior; the e2e test exists to catch a
clap upgrade changing it.
