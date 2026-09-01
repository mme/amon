//! Loading `carriers.toml`.
//!
//! The table ships inside the binary, so a malformed entry is a bug caught by
//! this crate's tests rather than something a user can arrive at. Entries for
//! an agent this build does not know, and patterns that do not compile, are
//! dropped: the feature going quiet on one agent is a better failure than the
//! wrapper refusing to start.

use super::Carrier;
use amon_detect::{identify_agent, Agent};
use regex::Regex;
use serde::Deserialize;
use std::sync::OnceLock;

pub(super) const TABLE: &str = include_str!("carriers.toml");

#[derive(Deserialize)]
pub(super) struct Table {
    #[serde(default)]
    pub(super) carrier: Vec<Entry>,
}

#[derive(Deserialize)]
pub(super) struct Entry {
    pub(super) agent: String,
    /// Where to look for proof that the agent's own prompt is on screen.
    /// Defaults to herdr's Claude-shaped input box.
    #[serde(default = "default_prompt_region")]
    pub(super) prompt_region: String,
    /// What that proof looks like. Absent means the region merely has to hold
    /// something, which is how a drawn box answers for itself.
    pub(super) prompt_pattern: Option<String>,
    pub(super) region: String,
    pub(super) pattern: String,
    /// Lines to skip over even when `pattern` matches them: an agent that
    /// marks its steps and its notices the same way needs the difference
    /// spelled out. Scanning continues past a rejected line.
    pub(super) reject: Option<String>,
}

fn default_prompt_region() -> String {
    "prompt_box_body".to_string()
}

/// The carriers for one agent, in file order.
///
/// A flat list rather than a map because `Agent` is herdr's type and does not
/// implement `Hash`; the table is short enough that it does not matter.
pub(super) fn for_agent(agent: Agent) -> impl Iterator<Item = &'static Carrier> {
    static LOADED: OnceLock<Vec<(Agent, Carrier)>> = OnceLock::new();
    LOADED
        .get_or_init(load)
        .iter()
        .filter(move |(theirs, _)| *theirs == agent)
        .map(|(_, carrier)| carrier)
}

/// `None` for a pattern that is present but will not compile, so the entry is
/// dropped whole rather than silently losing the half that guards it — a
/// carrier missing its reject would narrate the noise it was written to skip.
fn compile_optional(pattern: Option<&str>) -> Option<Option<Regex>> {
    match pattern {
        None => Some(None),
        Some(pattern) => Regex::new(pattern).ok().map(Some),
    }
}

fn load() -> Vec<(Agent, Carrier)> {
    let table: Table = toml::from_str(TABLE).expect("carriers.toml ships with this binary");
    table
        .carrier
        .into_iter()
        .filter_map(|entry| {
            let agent = identify_agent(&entry.agent)?;
            let pattern = Regex::new(&entry.pattern).ok()?;
            let prompt_pattern = compile_optional(entry.prompt_pattern.as_deref())?;
            let reject = compile_optional(entry.reject.as_deref())?;
            Some((
                agent,
                Carrier {
                    prompt_region: entry.prompt_region,
                    prompt_pattern,
                    region: entry.region,
                    pattern,
                    reject,
                },
            ))
        })
        .collect()
}
