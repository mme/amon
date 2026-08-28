//! Stands in for herdr's `crate::platform`.
//!
//! herdr's platform layer is mostly process-table scanning, which amon does
//! not need: it spawns the agent itself and knows what it launched (see
//! amon-detect's shim of the same name). The only piece the vendored terminal
//! code reaches for is the presentation cleanup for a terminal title, and that
//! exists for one Windows quirk — an elevated console prefixes the title with
//! "Administrator: ". Everywhere else the title is already what it should be.

#[cfg(not(windows))]
pub(crate) fn terminal_title_for_presentation(title: &str) -> &str {
    title
}

#[cfg(windows)]
pub(crate) fn terminal_title_for_presentation(title: &str) -> &str {
    title.strip_prefix("Administrator: ").unwrap_or(title)
}
