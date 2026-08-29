//! Stands in for herdr's `crate::events`.
//!
//! The vendored updater can report its results over a channel
//! (`manifest_update::auto_update`). amon's daemon calls the synchronous
//! `check_and_update` instead, so this exists to keep that signature
//! compiling — and to stay usable if we ever want the channel form.

use crate::detect::manifest_update::{ManifestUpdateCommit, ManifestUpdateStatus};
use crate::detect::Agent;

#[derive(Debug, Clone)]
pub enum AppEvent {
    AgentDetectionManifestsUpdated {
        updated: Vec<ManifestUpdateCommit>,
        /// Agents whose in-memory manifest was reloaded because the copy on
        /// disk moved out from under it. Upstream reports these so a running
        /// multiplexer can re-detect; amon's daemon updates synchronously and
        /// has nothing to invalidate, but the field is part of the event the
        /// vendored updater constructs.
        activated: Vec<Agent>,
        status: ManifestUpdateStatus,
    },
}
