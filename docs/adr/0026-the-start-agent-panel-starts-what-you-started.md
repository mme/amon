# The start agent panel starts what you started

Omarchy's default-agent key starts one agent, in one place, with flags of its
choosing. What a person actually wants back is narrower and more specific:
the agent they last ran in a given project. amon already sees every agent
start, so it can offer exactly that list.

## What it is

An overlay in the agent panel's style - modal only, never popped out - titled
"start agent", listing the agents you started yourself, most recent first.
Picking a row opens a new terminal in that folder running that agent, on the
current workspace, focused. `Super+Alt+A` toggles it (the free A chord: Super+A
is the agent panel, Shift is ChatGPT, Shift+Alt is Grok, Shift+Ctrl is
Omarchy's default agent, Ctrl is audio). Esc and Super+W close it. On the
Micro 2, `macro_2` defaults to the new `start` action, and a second press
closes the panel; the dial walks its rows and the click starts one. Omarchy's
own Super+Shift+Ctrl+A is left alone.

Only one amon overlay is up at a time: opening either panel closes the other.
A popped-out agent panel is a normal window and may sit beside it.

## What a row is

One per agent and folder, plus the host for a remote one. Five columns: the
agent; the project in bold - the repository's name, or outside one the
folder's own name; the branch it was on when last started; the folder dim,
with the home as `~` and a remote one led by its host, giving way from the
front when the row is short; and when it was last started:
"just now", "5 min ago", "2 h ago", "yesterday", then "Sep 28", or
"Sep 28 2025" from an earlier year. At most 25 rows. A local row whose
folder no longer exists is hidden, not forgotten. With no history the panel
says so: "No agents yet - start one in a terminal and it shows up here."

## What is recorded, and where

Recognised agents only - the kinds amon's detection knows - that amon wraps
in a terminal on this machine, recorded at launch; and agents on another
machine, recorded when they claim an ssh session (ADR-0023), which needs amon
on both ends and nothing on the ssh command line. Not agents inside herdr or
luvus (amon cannot open a pane for them), not other programs run through
amon, not launches without a terminal (ADR-0016).

`~/.local/state/amon/started.toml`, one `[[agent]]` table per entry - the
command, the folder, its project and branch, for a remote one the host and
the ssh connection arguments, and
when it was last started - pruned to the 25 most recent. TOML because people
edit it by hand, as they do the config: re-read every time the panel opens,
so an edit applies at once. Writers take a lock, so two agents starting at
the same moment cannot drop each other's entry.

## What a pick runs

The bare agent - `claude`, never `claude --resume` or the flags Omarchy's
launcher adds - because a row is a place to start from, not a session to
replay. Locally: a terminal with Omarchy's agent class in the folder, running
`amon <agent>`. Remotely: a terminal running `amon ssh -t <the same
arguments>` that changes to the folder there and starts the agent through the
remote login shell, so the remote side's own aliases and setup apply.
