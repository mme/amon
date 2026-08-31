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
    pub(super) region: String,
    pub(super) pattern: String,
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

fn load() -> Vec<(Agent, Carrier)> {
    let table: Table = toml::from_str(TABLE).expect("carriers.toml ships with this binary");
    table
        .carrier
        .into_iter()
        .filter_map(|entry| {
            let agent = identify_agent(&entry.agent)?;
            let pattern = Regex::new(&entry.pattern).ok()?;
            Some((
                agent,
                Carrier {
                    region: entry.region,
                    pattern,
                },
            ))
        })
        .collect()
}
