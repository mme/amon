# Super+N lands on the agent, and one ranking decides which

Omarchy binds `SUPER + code:10..19` to `hl.dsp.focus({ workspace = "N" })`.
That switches the workspace and leaves the focus wherever the compositor last
put it — usually not the agent you crossed the desktop to reach. The bar can
already say *which* workspace wants you (ADR-0008); arriving there and landing
on a browser is the last step of that answer going missing.

So `amon focus <n>` replaces those bindings, and the bar widget's click routes
through the same command. Three things follow from that, each of which cost
something to get right.

## The ranking moves onto the entry

Ordering used to be `AgentState::urgency()`: `Blocked > Working > Idle >
Unknown`, sorting `amon status`. It cannot express the state that most wants a
human, because that state is not a state. "Done" is `Idle` carrying `seen:
false` — finished and not yet looked at (ADR-0007) — an annotation deliberately
kept off the enum, which left every consumer to re-derive it. The bar had
invented its own placement for it; nothing checked the two against each other.

`AgentEntry::attention()` replaces it, over the entry: **blocked, done,
working, idle**, with `Unknown` last. Done above working is the substance of
the change, not a detail of it — work in progress asks nothing of anyone, work
that has finished and gone unseen is the likeliest thing on the machine to be
waiting on you. The registry sort, `amon status`, the bar and `amon focus` all
read the one function, so the bar and the keystroke can no longer disagree
about the same workspace.

The widget cannot call Rust. Its `order` array is therefore a second copy, and
a test pins it to `attention()` — the only thing that can.

## One dispatch, or you see the wrong window first

The obvious shape is: switch to the workspace, then focus the agent. It is
wrong, visibly, every time — you land on whatever was focused there and get
pulled off it a frame later.

Focusing a window switches to its workspace on the way, so resolving the agent
*first* and then issuing a single focus has no intermediate state to see. That
inverts the cost: the daemon query now sits between the keypress and anything
happening, which is why it is bounded hard (150ms) and why `amon focus` never
*starts* a daemon. A keybinding must not pay for a daemon launch, and a
workspace switch that stalls is a worse failure than one that lands on the
window the compositor would have picked anyway.

Two facts about Quattro were established by testing, not assumed, and both
shaped the code:

- **Classic dispatchers are gone.** `hyprctl dispatch focuswindow address:…`
  is a Lua syntax error there; `hyprctl dispatch` evaluates Lua now. The form
  is `hl.dsp.focus({ window = "address:0x…" })`, and `hl.focus` accepts only
  `direction, monitor, window, urgent_or_last, last`.
- **`hyprctl` exits 0 when it refuses.** A window that has closed since the
  daemon answered comes back `warning: window not found` on stdout with a zero
  status. The fallback therefore keys off stdout being `ok`, never the exit
  code — a mistake that would have silently stranded the user on the wrong
  workspace.

**Every failure degrades to the plain switch.** No daemon, a slow one, no agent
there, none that wants anything, an agent the compositor never gave a window
(ssh, tmux), a window that has since closed: all of them mean go to the
workspace and let Hyprland choose. Once amon owns Super+N, amon being broken
must never cost the user the ability to change workspace.

## The bindings live in two files, one of them the user's

Quattro's config is Lua. `hyprland.lua` requires Omarchy's defaults and then
the user's own modules, and the documented way to change a binding is
`hl.unbind` followed by `o.bind` in `~/.config/hypr/bindings.lua`. There is no
drop-in directory for it — the `~/.config/hypr/conf.d/*.conf` that `hyprland.conf`
sources belongs to the pre-Quattro `.conf` world and is not read by the Lua
path. `require_all.files` exists but every call site points at Omarchy's own
directories or at `~/.local/state/omarchy/toggles/hypr`, which is
`omarchy toggle`'s namespace and not ours to squat.

That leaves exactly one sanctioned extension point, and it is a file full of
someone's personal bindings. So the work is split:

- `~/.config/hypr/amon.lua` — amon's outright, rewritten or deleted without
  reading. All ten rebinds live here.
- `~/.config/hypr/bindings.lua` — gains one fenced line, `require("hypr.amon")`.
  Omarchy's bootstrap puts `$HOME/.config/?.lua` on the Lua module path, which
  is what makes that resolve.

The blast radius on the user's file is one line instead of twenty, and the
fence machinery is the shell rc's, generalized over comment syntax rather than
written twice: a truncated fence is refused rather than repaired, a symlinked
file is never written through (a dotfiles checkout is someone's repository),
and the block is spliced rather than appended so a second setup leaves one.

The Lua checks amon's binary exists before rebinding, and the path written in
is absolute. Both guard the same failure: these bindings run from the
compositor, whose `PATH` at session start is not the login shell's, and an amon
deleted without `amon remove` would otherwise take the workspace keys with it.
Losing Super+N is far worse than losing the jump.

## Consequences

Amon now edits a second file outside its own directories, and a more delicate
one than `.bashrc`: a config error there is a Hyprland that fails to load
bindings at login. Removal has to be exact, which is why `amon remove` takes
the require out *before* deleting the file it points at — the reverse order
leaves a require aimed at nothing.

Scope is Super+N and the bar click, deliberately. `SUPER + TAB`,
`SUPER + SHIFT + TAB` and `SUPER + CTRL + TAB` reach workspaces too and still
behave the old way; covering them means `amon focus` accepting relative targets
(`e+1`, `previous`), which is more surface for less traffic. `amon focus` takes
a `u32` for the same reason it is safe to interpolate: named and special
workspaces are out of scope, and a number cannot inject Lua. Window tokens are
checked to be hex before they are pasted into an expression — they are opaque
by contract and arrive over the socket.

The rebind is only as good as `hl.unbind` matching. It is written with the same
`SUPER + code:NN` string Omarchy binds with, so it matches today; an Omarchy
that changes how it spells those keys would leave amon's binding beside theirs
rather than in place of it.
