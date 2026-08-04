# An alias makes the agent's own name run it under amon

amon only sees an agent it started (ADR-0006), which means remembering to type
`amon` first. Forget once and that session is invisible: no state, no
workspace, nothing in the bar. So `amon install <agent>` also writes
`alias claude='amon claude'` into the user's `~/.bashrc`.

Adopting a session already running was considered first, and ruled out by
measurement rather than assumption. Reading another process' output after the
fact needs `ptrace` or eBPF; with `kernel.yama.ptrace_scope = 1` — the default
here — a non-ancestor is refused outright, so amon would need a machine-wide
policy change or `CAP_SYS_PTRACE` on its own binary. A process *can* volunteer
with `prctl(PR_SET_PTRACER)`, and that works, but the grant is **not inherited
across `fork`**, so it cannot be set once in a shell and picked up by the agents
that shell starts. Whatever the route, output written before attaching is gone,
and a mid-stream attach yields a byte stream against a screen never seen —
detection needs the screen, not the bytes. The moment to act is launch, and at
launch the wrapper is available, needs no privileges, and misses nothing.

## An alias, not a script on PATH

The alternative was a small script named after the agent, ahead of the real one
on `PATH`. It reaches further: it would also cover `foot -e claude`, an editor,
or a Justfile, none of which involve an interactive shell. It was built, and
then dropped in favour of the alias, which is a few lines instead of a
generated program and cannot recurse — `PATH` lookup never consults aliases, so
the `claude` inside `alias claude='amon claude'` is the real one. A script has
to resolve the real agent itself, skipping its own directory, or amon's own
`PATH` search finds the script again.

`~/.local/bin` would have been the wrong place for such a script in any case:
that is where several agents install *themselves* — on Omarchy `claude` and
`codex` are both there — so a script of that name would not shadow the agent
but overwrite it.

## Consequences

**Interactive shells only.** Bash does not expand aliases in scripts or in
`sh -c`, so an agent started by a keybinding, an editor task, or `foot -e
claude` runs unwrapped. That is the price of the simpler mechanism, and the
prefix still works everywhere.

**If amon is missing, the agent does not start.** The alias names a command
that is not there and bash says so. This is loud, immediate, and worked around
by `command claude` — unlike a `PATH` script, which would have had to carry its
own fallback so that a broken amon could not stop an agent from running.

**It is opt-out, not opt-in.** `amon install <agent>` writes the alias and
`--no-alias` declines. Redefining a command name is a surprising thing to find
later, so install prints the line it wrote, and `amon uninstall <agent>` takes
it away again.

**amon edits a file it does not own.** `~/.bashrc` is the user's. amon writes
one fenced block, rewrites only what is between the fences, and leaves a
`~/.bashrc` that is a symlink alone entirely — following one would edit a file
in somebody's dotfiles repository — printing the lines to add by hand instead.
Bash only: it is Omarchy's default and the only shell installed there, and a
line written for a shell the user does not run would be worse than none.
