# Setup is an interactive reconciler that runs the omarchy CLI

`amon install <target>` / `amon uninstall <target>` become `amon setup` /
`amon remove`. Bare `amon setup` on a TTY is an inline checklist — one row per
Detected Agent plus, when the `omarchy` CLI is on PATH, the bar widget. The
screen *reconciles*: checked rows are set up or refreshed, unchecking an
installed row removes that Integration, and Enter applies immediately. Rows are
ordered by Omarchy's own default-agent menu order (mirrored as a data table,
never parsed from their script at runtime), with the system default agent —
read from `omarchy-default-agent` — first, and agents Omarchy does not know
last. Agents that are neither detected nor installed do not appear; an
installed Integration whose Agent has vanished appears flagged, so the one
screen can always remove what exists.

Three decisions in that screen reverse earlier stances and are recorded here so
nobody "fixes" them back:

**Setup executes the omarchy CLI instead of printing commands.** Until now
install printed `omarchy plugin enable …` for the user to run, because amon
refused to edit bar state it does not own. The refusal stands — `shell.json` is
still only ever written by Omarchy's own tooling — but an interactive flow that
ends in "now go type this" is the friction the flow exists to remove. With the
widget's manifest declaring `clonedFrom` (issue #3), enable/disable is a single
idempotent command with automatic restore, so the blast radius that justified
printing is gone. Non-interactive invocations still print, unless `--all` makes
the intent explicit.

**Apply is immediate, with no confirmation diff.** Unchecking an installed row
deletes hooks and un-aliases an agent the moment Enter lands. A summary-then-
confirm step was considered and declined: the selection screen *is* the
confirmation, and everything it removes is cheaply reinstallable by running
setup again. This is deliberate, not an oversight.

**One failed item does not stop the rest.** Each row applies independently;
failures show inline with the error, the exit code is non-zero, and the final
"open a new shell and run your agent" hint still prints when anything
succeeded. A wedged `omarchy plugin enable` must not hold agent hooks hostage —
the same posture the Wrapper takes toward a wedged Daemon.

Around the screen: `amon remove` is all-or-nothing (list what is installed, one
confirmation, remove everything — partial removal lives in `amon remove
<target>` and in the setup screen); without a TTY, `amon setup` errors and
names the flags rather than assuming `--all`; and `amon integrations` becomes
`amon doctor`, which grows from a listing into a health check (integration
currency, daemon reachability and version, socket, omarchy CLI, widget actually
in the bar, aliases actually in `~/.bashrc`). The amon binary itself is never
setup's job: packages ship `/usr/bin/amon` and their post-install prints "run
`amon setup`" — pacman scriptlets run as root and may neither touch `$HOME` nor
prompt, which is exactly the boundary between the package's half and setup's
half.

## Postscript: what applying the screen does, and what has no row

The body describes the screen as "one row per Detected Agent plus, when the
`omarchy` CLI is on PATH, the bar widget". The widget's row is gone, and it was
not replaced by another one. On a machine with the `omarchy` CLI, applying the
screen also installs the bar widget (ADR-0008) and the Super+N bindings
(ADR-0011) — neither is a question, because neither is a choice worth putting
to someone who has just said yes to setting amon up. `amon setup --all` does
the same. Taking either back off is `amon remove`'s job, which is the one
capability this costs: there is no way to have amon's agent hooks and decline
its desktop half short of removing it afterwards.

The reasoning is the same one the body applies to the agent rows. A row is a
question, and a question implies an answer worth having. "Do you want the bar
to show agent state" and "do you want Super+N to land on the agent" are not
questions a person setting up amon on Omarchy is served by; they are what
setting amon up on Omarchy *means*. What earned a row — which agents to hook,
and therefore whose name gets aliased (ADR-0009) — is genuinely per-agent and
genuinely reversible.

"One failed item does not stop the rest" now spans three kinds of item rather
than two, and one of them edits a file amon does not own. A refused
`bindings.lua` — truncated fence, symlinked into a dotfiles repository — is
reported as a note and the run continues; it is not an error, because there is
nothing wrong except that amon has correctly declined to touch it.
