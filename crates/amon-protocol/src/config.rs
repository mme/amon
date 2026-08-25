//! What the user's config file says, as it travels the wire.
//!
//! The file itself is TOML the daemon reads; these are the parsed answers, and
//! they go to subscribers over the socket they already hold (ADR-0012). The
//! widget therefore never learns where the file lives, never parses TOML, and
//! cannot disagree with the daemon about what it says.
//!
//! Every field is optional and every absent field means "the consumer's own
//! default". Defaults deliberately do not live here: the bar's glyphs belong to
//! the bar, and a daemon that shipped its own copy of them would be a second
//! opinion about how the widget should look.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The whole of the user's configuration.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct Config {
    pub bar: BarConfig,
    pub sound: SoundConfig,
    pub updates: UpdatesConfig,
}

/// What the workspace indicators draw, and how fast.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct BarConfig {
    /// How long one spinner frame is shown, in milliseconds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub frame_ms: Option<u32>,
    /// Beats of the focused workspace's turn-taking spent showing the agent's
    /// state. A beat is half a turn of the spinner (ADR-0008).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state_beats: Option<u32>,
    /// Beats spent showing the focus marker.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub marker_beats: Option<u32>,
    pub glyphs: GlyphConfig,
}

/// Which character stands for each condition.
///
/// Plain characters, never markup: this chooses *which* glyph, never how it is
/// drawn. A workspace at rest keeps the underline the widget computes for
/// itself, and nothing here can reach it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct GlyphConfig {
    /// The animation frames for a working agent, in order. Any length; one
    /// frame is a static glyph that happens to be redrawn.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub working: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub blocked: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub done: Option<String>,
    /// The marker for the workspace you are on.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub focused: Option<String>,
}

/// When to make a noise, and which one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct SoundConfig {
    /// On unless turned off.
    ///
    /// This started as off-by-default, reasoning that a tool which begins
    /// making noise on upgrade is a tool people turn off entirely. The counter
    /// is stronger: the whole point is to tell you about an agent you are *not*
    /// watching, and a notification nobody knows exists notifies nobody. The
    /// two sounds are rare — an agent finishing unwatched, or starting to wait
    /// for you — and the config file `amon setup` writes shows how to silence
    /// them.
    pub enabled: bool,
    /// Played when an agent finishes without being watched. Absent means the
    /// bundled sound; a relative path resolves from the config file's own
    /// directory, the way herdr's does.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub done: Option<String>,
    /// Played when an agent starts waiting for a human.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub blocked: Option<String>,
}

impl Default for SoundConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            done: None,
            blocked: None,
        }
    }
}

/// Whether the daemon may look for newer releases.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct UpdatesConfig {
    /// On unless turned off. The whole of the check is one HEAD request
    /// against the release page's stable redirect, at most daily; nothing
    /// about this machine travels with it.
    pub check: bool,
}

impl Default for UpdatesConfig {
    fn default() -> Self {
        Self { check: true }
    }
}
