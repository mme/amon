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
    "-- Added by amon, so Super+N lands on the agent that wants you rather than",
    "-- on whatever was focused last. Deleting this block undoes it.",
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
fn lua(binary: &str) -> String {
    format!(
        r#"-- Managed by amon. `amon setup` rewrites this file; `amon remove` deletes it.
--
-- Super+N goes to a workspace and lands on the agent that most wants a human
-- there: blocked first, then finished-but-unseen, then working. An agent at
-- rest is never jumped to, and a workspace without one is the plain switch
-- Omarchy binds by default — which is also what `amon focus` falls back to
-- when the daemon cannot answer.
--
-- Bound by keycode the way Omarchy binds them (code:10 is the `1` key), so
-- these replace those bindings instead of sitting beside them.

local amon = {path}

-- Only rebind if that binary is really there. An amon deleted without
-- `amon remove` would otherwise take Super+N with it, and losing the ability
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
"#,
        path = lua_string(binary),
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
    std::fs::write(&ours, lua(binary))?;
    // The require last, as with the widget's manifest: until it is there the
    // Lua file is inert, so a write that fails part way leaves nothing loaded
    // rather than a require pointing at a file that is not finished.
    let block = FENCE.block(&PREFACE, &[REQUIRE_LINE.to_string()]);
    std::fs::write(&theirs, FENCE.with_block(&existing, &block))?;

    Ok(vec![
        "Super+N now lands on the agent that wants you".to_string()
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
        Ok(()) => notes.push("Super+N goes back to Omarchy's own binding".to_string()),
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
