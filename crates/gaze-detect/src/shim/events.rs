//! Stands in for herdr's `crate::events`.
//!
//! The vendored updater can report its results over a channel
//! (`manifest_update::auto_update`). gaze's daemon calls the synchronous
//! `check_and_update` instead, so this exists to keep that signature
//! compiling — and to stay usable if we ever want the channel form.

use crate::detect::manifest_update::{ManifestUpdateCommit, ManifestUpdateStatus};

#[derive(Debug, Clone)]
pub enum AppEvent {
    AgentDetectionManifestsUpdated {
        updated: Vec<ManifestUpdateCommit>,
        status: ManifestUpdateStatus,
    },
}
