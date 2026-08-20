//! The config file `amon setup` leaves behind the first time.
//!
//! Everything amon can be configured with has a default, so the file is not
//! needed for anything to work (ADR-0012). It exists to be *found*: a setting
//! nobody knows about configures nothing, and the alternative — reading an ADR
//! to learn that glyphs can be changed — is not a discoverable interface.
//!
//! Every line is commented out, so writing it changes no behaviour. It is
//! documentation that happens to live where the documentation would be edited.

use std::io;

/// Written once, never rewritten.
///
/// The moment this file exists it is the user's, and `amon setup` runs often —
/// refreshing hooks, adding an agent. Rewriting a file someone has edited,
/// however good the intention, is the behaviour a config file must never have.
pub fn write_if_absent() -> io::Result<Option<std::path::PathBuf>> {
    let Some(path) = amon_daemon::config_path() else {
        return Ok(None);
    };
    if path.exists() {
        return Ok(None);
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, STARTER)?;
    Ok(Some(path))
}

const STARTER: &str = r##"# amon
#
# Everything here is optional and everything is commented out, so this file
# changes nothing until you edit it. The values shown are the defaults.
#
# Saved changes apply within about a second. Nothing needs restarting.

[bar]
# How long one frame of the spinner is shown, in milliseconds.
# frame_ms = 200

# The focused workspace takes turns between your agent's state and the marker
# that says you are here, counted in beats of half a spinner turn.
# state_beats = 5
# marker_beats = 3

[bar.glyphs]
# Plain characters: this chooses which glyph, never how it is drawn. A
# workspace whose agent is at rest keeps its number, underlined.
# working = ["⠒", "⠰", "⠤", "⠆"]
# blocked = "󰋗"
# done = "󰗠"
# focused = "󱓻"

[sound]
# A short sound when an agent starts waiting for you, and when one finishes
# while you were not looking at it. Nothing plays for an agent you are watching.
#
# On by default. Uncomment to silence both:
# enabled = false

# Your own sounds instead of the bundled ones. A relative path is resolved from
# this file's own directory.
# done = "~/sounds/finished.mp3"
# blocked = "~/sounds/attention.mp3"
"##;
