# Hyprland's Lua API, and what it means for amon

Research notes, 2026-07-31. Everything below marked **verified** was executed
live against the `omarchy-quattro` dev VM (`ssh quattro`) running **Hyprland
0.56.1** (commit `5c9377c15f85c50648f35ca5a213754f95b93ca0`, tag v0.56.1, built
2026-07-27), and contrasted with the host desktop running **Hyprland 0.56.0**,
which still behaves the old way. The sources here are the running compositor
itself rather than URLs; §6 lists what a primary-source pass still owes.

## TL;DR

Newer Hyprland replaces the string dispatcher syntax with a **Lua API**, and
it does so **at the IPC layer, not just in `hyprctl`** — writing
`dispatch focuswindow address:0x…` to `.socket.sock` now fails to parse. The
half amon depends on is untouched: `j/clients` and the `.socket2.sock` event
stream are byte-for-byte what they were, so **amon works on Quattro as-is**
(verified end to end, §4). The cost lands on the deferred "jump to the agent's
window" feature, which will need version-aware dispatch (§5).

## 1. The old dispatch syntax is gone — over the socket too

**Verified.** Both of these fail on 0.56.1, and identically whether sent via
`hyprctl` or written straight to `$XDG_RUNTIME_DIR/hypr/$SIG/.socket.sock`:

```
dispatch focuswindow address:0x5635b755fcf0
  → error: [string "return hl.dispatch(focuswindow address:0x5635…"]:1:
    ')' expected near 'address'
    → Note: dispatch in lua is a shorthand for hl.dispatch(...),
      your syntax might need to be updated.

dispatch movetoworkspacesilent 3,address:0x5635b755fcf0
  → error: … ')' expected near '3'
```

The whole argument string is wrapped as `return hl.dispatch(<your text>)` and
evaluated as Lua. This is the finding that matters for any program: a Rust
client that prefers the socket over shelling out gains nothing by avoiding
`hyprctl` — the Lua requirement is in the compositor.

## 2. The API surface, by introspection

The compositor will enumerate itself if you make it raise the answer as an
error. This trick is how everything below was obtained:

```sh
hyprctl dispatch 'error(table.concat((function()
  local t={} for k in pairs(hl.dsp) do t[#t+1]=k end
  table.sort(t) return t end)(), " "))'
```

**Verified** contents:

- **`hl`** (top level): `animation bind clear_crashed_lockscreen config curve
  define_submap device dispatch dsp env exec_cmd
  exec_scheduled_prop_refresh_immediately gesture get_active_monitor
  get_active_special_workspace get_active_window get_active_workspace
  get_config get_current_submap get_cursor_pos get_last_window
  get_last_workspace get_layers get_loaded_plugins get_monitor get_monitor_at
  get_monitor_at_cursor get_monitors get_urgent_window get_window get_windows
  get_workspace get_workspace_windows get_workspaces is_key_down layer_rule
  layout monitor notification on permission plugin timer unbind version
  window_rule workspace_rule`
- **`hl.dsp`** (dispatchers): `cursor dpms event exec_cmd exec_raw exit focus
  force_idle force_renderer_reload global group layout no_op pass
  release_input_capture send_key_state send_shortcut submap window workspace`
- **`hl.dsp.window`**: `alter_zorder bring_to_top center clear_tags close
  cycle_next deny_from_group drag float fullscreen fullscreen_state kill move
  pin pseudo resize set_prop signal swap tag toggle_swallow`
- **`hl.dsp.workspace`**: `change_id move rename swap_monitors toggle_special`
- Types: `hl.dispatch` is a function, `hl.dsp` a table, **`hl.dsp.focus` a
  function** (not a table — `hl.dsp.focus.window(…)` is a type error), and
  `hl.dsp.window.close` a function.

This is a configuration and scripting surface, not merely renamed dispatchers:
`on` (event subscription), `bind`/`unbind`, `window_rule`, `timer`, `plugin`,
and a family of `get_*` queries all live in-process. None of it is reachable
from another process — an external program still uses the sockets.

## 2.1 The errors are the documentation

Wrong arguments produce the schema, which is the fastest way to a working
call:

```
hl.dsp.focus()                      → hl.focus: expected a table, e.g. { direction = "left" }
hl.dsp.focus({address="0x…"})       → hl.focus: unrecognized arguments.
                                       Expected one of: direction, monitor, window,
                                       urgent_or_last, last
hl.dsp.focus({window={address="…"}}) → hl.focus: expected a window object or selector
hl.dsp.workspace.move({id=1})       → hl.workspace.move: 'monitor' is required
hl.dsp.workspace.change_id({id=1})  → hl.workspace.change_id: 'workspace' is required
```

## 3. Calls that work

**Verified**, each by its observable effect, not by its return value — `ok` is
returned even when nothing happens (see `focus` below):

| Intent | Call | Result |
|---|---|---|
| Close a window | `hl.dsp.window.close({window="address:0x…"})` | Window destroyed, `closewindow>>…` emitted. A real close, exactly as if the user had closed it — no softer programmatic variant exists. |
| Move a window to a workspace | `hl.dsp.window.move({window="address:0x…", workspace="3"})` | Window moved; events in §4 |
| Renumber a workspace | `hl.dsp.workspace.change_id({workspace="3", id=1})` | Workspace 3 became 1 |
| Focus a window | `hl.dsp.focus({window="address:0x…"})` | Returns `ok`, **no observable effect** — see below |

The **window selector is a string** in the old familiar form,
`"address:0x5635b755fcf0"`; a nested table (`{window={address=…}}`) is
rejected. `hl.get_window("address:0x…")` accepts the same string and its
result is also accepted as `window`, so both of these are valid:

```lua
hl.dsp.focus({window="address:0x5635b755fcf0"})
hl.dsp.focus({window=hl.get_window("address:0x5635b755fcf0")})
```

**Focus could not be confirmed to work.** Both forms return `ok`, but the
active window stays empty and *no event is emitted*, while `window.close` on
the very same selector works and does emit. **Inference:** the VM's session has
no seat/keyboard focus to give (its only monitor reports `focused: true`, but
`j/activewindow` returns `{}` even with windows mapped), so this is likely an
environment limitation rather than wrong syntax — unproven either way, and the
thing to re-test on a desktop session before relying on it.

## 4. What amon reads is unchanged

**Verified on 0.56.1.** The query socket answers `j/clients` with the same
JSON — `address` carrying the `0x` prefix, plus `pid` and
`workspace: {id, name}` — and the event stream still speaks the classic
`name>>payload` lines. A single window move produced, in order:

```
createworkspace>>3
createworkspacev2>>3,3
movewindow>>5635b755fcf0,3
movewindowv2>>5635b755fcf0,3,3
workspace>>3
workspacev2>>3,3
destroyworkspace>>1
destroyworkspacev2>>1,1
```

That is exactly what `hypr.rs` parses: addresses **without** the `0x` that the
query socket adds, `movewindow` carrying `address,name` and `movewindowv2`
carrying `address,id,name`. A close emits `closewindow>>5635b755fcf0`, also
unprefixed.

**End-to-end check:** an agent wrapped in a `foot` window on the VM resolved
to `window: 5635b755fcf0, workspace: 1`, and after the move above `amon
status` reported `workspace: 3`. The whole chain — ancestry lookup, event
follow, patch, daemon, status — works on the Lua-era compositor with no
changes.

## 5. What this costs amon later

Nothing today: amon only ever *reads*. Its sole write to the compositor is the
literal string `j/clients` on the query socket; it never dispatches.

The deferred jump-to-window feature (`goto`, see
[ADR-0007](../adr/0007-wrapper-enables-terminal-focus-reporting.md) discussion
and the CONTEXT glossary's **Window**) is where the split lands. It will need
version-aware dispatch over the same socket:

```
old (≤ 0.56.0):  dispatch focuswindow address:0x5635b755fcf0
new (0.56.1+):   dispatch hl.dsp.focus({window="address:0x5635b755fcf0"})
```

Both are one write to `.socket.sock`. Since the old form fails loudly with a
parse error rather than silently, trying one and falling back on error is
viable; `hyprctl version` (or `j/version`) would let it be decided up front.
Note the failed-focus caveat in §3 before assuming the new form is sufficient.

## 6. Not established

The primary-source pass was cut short, so these remain open — none of them
block amon, but they are what a follow-up should answer from
`github.com/hyprwm/Hyprland` and `github.com/basecamp/omarchy`:

- Which Hyprland release introduced the Lua API, and whether the string
  dispatcher form is deprecated, removed, or available behind a compatibility
  option. (The host at 0.56.0 still accepts it and the VM at 0.56.1 does not,
  which narrows it to that boundary — but the VM may be a non-stock build,
  since Omarchy pins its own.)
- The authoritative Lua binding definitions and argument schemas, rather than
  the shapes recovered from error messages here.
- What "Omarchy Quattro" is exactly — release, branch, or distro spin — and
  which Hyprland it pins.
- Whether the event protocol gains anything in the Lua era (a v3 scheme, new
  event names) that amon might want.

## 7. Reproducing this

The VM is a full Hyprland session, which the host cannot safely stand in for —
testing window tracking on the host means moving the user's real windows.

```sh
ssh quattro
export HYPRLAND_INSTANCE_SIGNATURE=$(ls /run/user/1000/hypr | head -1)
export WAYLAND_DISPLAY=wayland-1          # the VM's only Wayland socket
setsid foot -e ~/run-amon.sh >/dev/null 2>&1 </dev/null &   # a window to test with
hyprctl -j clients
```

Two traps worth remembering. `hyprctl dispatch exec` cannot launch the
terminal on this version (its argument is Lua), so launch `foot` directly with
`WAYLAND_DISPLAY` set. And `pgrep -x amon | head -1` will happily hand back an
orphan from a previous run — select the process by walking parents up to the
`foot` pid instead, or the readings are someone else's.

The repo is cloned at `~/Projects/amon.sh` on the VM; `mise` there already
pins zig 0.15.2 and cargo-nextest, so `cargo build` and `cargo nextest run`
work after an `rsync -a --exclude target/ ./ quattro:Projects/amon.sh/`.
