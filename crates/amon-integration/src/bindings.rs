//! The Super+N bindings, so arriving at a workspace lands on its agent.
//!
//! Omarchy binds `SUPER + code:10..19` to a plain workspace switch, which
//! leaves you on whatever window was focused there last — usually not the
//! agent you went looking for. Amon rebinds those ten keys to `amon focus`,
//! which resolves the agent first and then makes exactly one jump.
//!
//! Split across two files on purpose. The bindings themselves live in
//! `~/.config/hypr/amon.lua`, which amon owns outright and may rewrite or
//! delete without reading; the user's own `bindings.lua` gains a single fenced
//! `require`. Quattro's config is Lua, and its only user extension point is
//! that hand-authored file — there is no `conf.d` for it — so the blast radius
//! on a file full of someone's personal bindings is one line rather than
//! twenty.
//!
//! `~/.config` literally, not `XDG_CONFIG_HOME`: Omarchy's bootstrap builds
//! its Lua module path as `$HOME/.config/?.lua` and never consults the
//! variable, so honouring it here would write where `require` will not look.

use std::io;
use std::path::PathBuf;

use crate::fence;

const FENCE: fence::Fence = fence::DASH;

/// What the user's own bindings file gains — one line, and the markers saying
/// which line is not theirs.
const PREFACE: [&str; 3] = [
    "-- Added by amon, so Super+0-9 lands on the agent that needs your attention",
    "-- rather than on whatever was focused last. Deleting this block undoes it.",
    "-- amon rewrites what is between these two markers and nothing else here.",
];

/// The module `require` resolves to `~/.config/hypr/amon.lua`.
const REQUIRE_LINE: &str = "require(\"hypr.amon\")";

fn home() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .filter(|home| !home.is_empty())
        .map(PathBuf::from)
}

fn missing_home() -> io::Error {
    io::Error::other("cannot find the home directory: HOME is not set")
}

/// The file amon owns outright.
pub fn amon_lua() -> Option<PathBuf> {
    home().map(|home| home.join(".config/hypr/amon.lua"))
}

/// The user's own bindings, which amon only ever adds one fenced line to.
pub fn bindings_lua() -> Option<PathBuf> {
    home().map(|home| home.join(".config/hypr/bindings.lua"))
}

/// Whether the bindings are in place — both halves, since either alone does
/// nothing.
pub fn installed() -> bool {
    let (Some(ours), Some(theirs)) = (amon_lua(), bindings_lua()) else {
        return false;
    };
    ours.is_file()
        && std::fs::read_to_string(theirs)
            .map(|existing| existing.contains(REQUIRE_LINE))
            .unwrap_or(false)
}

/// Escapes a path into a Lua string literal.
fn lua_string(value: &str) -> String {
    let mut out = String::from("\"");
    for character in value.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            other => out.push(other),
        }
    }
    out.push('"');
    out
}

/// Wraps a path so a shell run by `exec_cmd` treats it as one word, whatever
/// is in it.
fn shell_quoted(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

/// The whole of `amon.lua`, pinned to the binary that wrote it.
///
/// An absolute path rather than a bare `amon`: these bindings run from the
/// compositor, whose `PATH` at session start is not the one the user's shell
/// has, and a binding that silently resolves to nothing would take the
/// workspace keys with it.
fn lua(binary: &str, shims: &str) -> String {
    format!(
        r#"-- Managed by amon. `amon setup` rewrites this file; `amon remove` deletes it.
--
-- Super+0-9 goes to a workspace and lands on the agent that most wants a human
-- there: blocked first, then finished-but-unseen, then working. An agent at
-- rest is never jumped to, and a workspace without one is the plain switch
-- Omarchy binds by default — which is also what `amon focus` falls back to
-- when the daemon cannot answer.
--
-- Bound by keycode the way Omarchy binds them (code:10 is the `1` key), so
-- these replace those bindings instead of sitting beside them.

local amon = {path}

-- Omarchy launches the default agent through a terminal's -e, which execs the
-- binary directly: no shell, so no alias, so amon never sees it. This puts a
-- directory of stand-ins for the agents you set up ahead of the real ones, so
-- that launch runs under amon too (ADR-0013).
--
-- Stripped before it is prepended, and that is not tidiness. Hyprland's own
-- environment carries what earlier `hl.env` calls set, and `os.getenv` here
-- reads it back — so a plain prepend appends another copy on every config
-- reload. Measured: four reloads, four copies. Omarchy's `envs.lua` strips its
-- own bin directory for exactly this reason, which is why it appears once.
local shims = {shim_dir}
local kept = {{}}
for entry in (os.getenv("PATH") or ""):gmatch("[^:]+") do
  if entry ~= shims then table.insert(kept, entry) end
end
table.insert(kept, 1, shims)
hl.env("PATH", table.concat(kept, ":"))

-- Only rebind if that binary is really there. An amon deleted without
-- `amon remove` would otherwise take those keys with it, and losing the ability
-- to change workspace is a far worse failure than losing the jump-to-agent.
local binary = io.open(amon, "r")
if binary then
  binary:close()

  for workspace = 1, 10 do
    local key = "SUPER + code:" .. tostring(workspace + 9)
    -- Unbound first: Omarchy already bound this key, and binding it twice
    -- would leave both in place.
    hl.unbind(key)
    o.bind(key, "Switch to workspace " .. workspace, {command} .. " focus " .. workspace)
  end
end

-- Super+A opens the agent panel. Bound whether or not amon's binary is there:
-- the pane is a shell plugin, and the shell is what answers this — `amon remove`
-- takes the plugin and this file together, so there is no state where the key
-- outlives the thing it opens.
--
-- The bare key, deliberately. Omarchy leaves A unbound while spending every
-- modified form of it — SHIFT+A is ChatGPT, SHIFT+ALT+A is Grok, SHIFT+CTRL+A
-- launches the default agent, CTRL+A is audio — so the unmodified one is the
-- slot left for the thing you reach for most.
hl.unbind("SUPER + A")
o.bind("SUPER + A", "Agents", "omarchy-shell shell toggle sh.amon.panel")

-- Omarchy fades every layer surface in and out — `looknfeel.lua` gives both
-- `layersIn` and `layersOut` a fade — and then exempts its own panes by
-- namespace in `apps/omarchy-shell.lua`. The clipboard, the emoji picker and
-- the menu are all on that list, which is the whole reason they open instantly
-- while an identically written pane does not. This is that same exemption, and
-- it is copied in the same shape, both fields and all: `animation = "none"`
-- on its own was tried first and left the fade exactly as it was.
--
-- The namespace has to match `WlrLayershell.namespace` in AgentPanel.qml, and is
-- anchored so it matches that surface and nothing else. A test holds the two
-- together, because a rule naming a surface that does not exist fails silently
-- by simply not applying.
hl.layer_rule({{ match = {{ namespace = "^amon-panel$" }}, no_anim = true, animation = "none" }})

-- The pane, popped out into a window of its own. Every Quickshell window shares
-- the class `org.quickshell`, so the title is the only thing that can single
-- this one out — it is matched exactly, and a test holds it to the title
-- AgentPanel.qml sets.
--
-- Floating and pinned from the moment it maps, which is what makes the
-- picture-in-picture icon on it honest: it sits above the tiling and follows
-- you across workspaces. Hyprland refuses to pin a window that is fullscreen,
-- and refuses to pin one that is tiled, so `float` here is not decoration —
-- without it the `pin` is declined and nothing says so.
--
-- Super+O is the way back: `omarchy-hyprland-window-pop` branches on whether a
-- window is already pinned, so the key that pops other windows out puts this
-- one away.
hl.window_rule({{
  match = {{ class = "org.quickshell", title = "^amon — agents$" }},
  float = true,
  pin = true,
  center = true,
}})
"#,
        path = lua_string(binary),
        shim_dir = lua_string(shims),
        command = lua_string(&shell_quoted(binary)),
    )
}

/// Writes both halves, returning the notes to print.
///
/// Idempotent: `amon.lua` is rewritten wholesale and the `require` is spliced
/// rather than appended, so running setup twice leaves one of each.
pub fn install() -> io::Result<Vec<String>> {
    let ours = amon_lua().ok_or_else(missing_home)?;
    let theirs = bindings_lua().ok_or_else(missing_home)?;
    let binary = std::env::current_exe()?;
    let binary = binary.to_str().ok_or_else(|| {
        io::Error::other("amon's own path is not valid UTF-8, so it cannot be written into Lua")
    })?;

    let existing = std::fs::read_to_string(&theirs).unwrap_or_default();
    if FENCE.unterminated(&existing) {
        return Ok(vec![FENCE.truncated_note(&theirs)]);
    }
    if fence::is_symlink(&theirs) {
        return Ok(manual_note(&theirs));
    }

    if let Some(parent) = ours.parent() {
        std::fs::create_dir_all(parent)?;
    }
    // The shim directory is named here even when it holds nothing yet: a
    // `PATH` entry pointing at an empty or absent directory costs nothing, and
    // it means setting up an agent later does not need the Hyprland config
    // rewritten to take effect.
    let shims = crate::shims::dir()
        .ok_or_else(missing_home)?
        .to_string_lossy()
        .into_owned();
    std::fs::write(&ours, lua(binary, &shims))?;
    // The require last, as with the widget's manifest: until it is there the
    // Lua file is inert, so a write that fails part way leaves nothing loaded
    // rather than a require pointing at a file that is not finished.
    let block = FENCE.block(&PREFACE, &[REQUIRE_LINE.to_string()]);
    std::fs::write(&theirs, FENCE.with_block(&existing, &block))?;

    Ok(vec![
        "Super+0-9 now lands on the agent that needs your attention".to_string(),
    ])
}

/// Takes both halves back out.
pub fn uninstall() -> io::Result<Vec<String>> {
    let ours = amon_lua().ok_or_else(missing_home)?;
    let theirs = bindings_lua().ok_or_else(missing_home)?;
    let mut notes = Vec::new();

    // The require first, mirroring install: a require left pointing at a
    // deleted file is a Hyprland config error at every login.
    let existing = std::fs::read_to_string(&theirs).unwrap_or_default();
    if FENCE.unterminated(&existing) {
        return Ok(vec![FENCE.truncated_note(&theirs)]);
    }
    if let Some(updated) = FENCE.without_block(&existing) {
        if fence::is_symlink(&theirs) {
            notes.push(format!(
                "{} is a symlink, so amon has not edited it — remove its amon block by hand",
                theirs.display()
            ));
        } else {
            std::fs::write(&theirs, updated)?;
        }
    }

    match std::fs::remove_file(&ours) {
        Ok(()) => notes.push("Super+0-9 goes back to Omarchy's own bindings".to_string()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    Ok(notes)
}

/// What to print when the user's bindings file is a symlink — a dotfiles
/// checkout linked into place, whose repository is not amon's to write to.
fn manual_note(theirs: &std::path::Path) -> Vec<String> {
    let mut notes = vec![format!(
        "{} is a symlink, so amon has not edited it — add this line yourself:",
        theirs.display()
    )];
    notes.extend(
        FENCE
            .block(&PREFACE, &[REQUIRE_LINE.to_string()])
            .lines()
            .map(|line| format!("  {line}")),
    );
    notes
}
