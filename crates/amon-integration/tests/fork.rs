//! The Omarchy widget is a fork, and forks drift (ADR-0008).
//!
//! The rule the ADR states is that the only difference from upstream is what a
//! workspace's label says. That is only enforceable against a copy of upstream,
//! so one is checked in beside this test — not installed, not compiled in, just
//! what the fork was derived from.
//!
//! Re-syncing means replacing both: the fixture with the new upstream file, and
//! the shipped widget with upstream plus the three marked changes.

/// Omarchy Quattro's `shell/plugins/bar/widgets/Workspaces.qml`, taken
/// 2026-08-02.
const UPSTREAM: &str = include_str!("fixtures/omarchy-Workspaces.qml");

/// What amon ships: upstream, saying something else on the buttons.
const FORK: &str = include_str!("../src/assets/omarchy/Workspaces.qml");

/// The two upstream lines the fork is allowed to replace rather than keep.
///
/// A plugin has to name itself and two plugins cannot share an id, so
/// `moduleName` must change. `text` is the feature: what a workspace displays
/// is computed in that expression from an `int`, and the widget reads no
/// settings, so there is no other seam at which a workspace could be made to
/// show its agent state.
const REPLACED: [&str; 2] = ["moduleName:", "text:"];

#[test]
fn the_fork_keeps_every_upstream_line_it_does_not_replace() {
    // Every other upstream line must still be there, in order. Insertions are
    // expected; a changed or deleted line is drift, and turns the next re-sync
    // from a re-application into a merge.
    let mut fork = FORK.lines().map(str::trim);

    for expected in UPSTREAM.lines().map(str::trim) {
        if expected.is_empty() || REPLACED.iter().any(|prefix| expected.starts_with(prefix)) {
            continue;
        }
        assert!(
            fork.any(|line| line == expected),
            "upstream line is missing from the fork, or out of order:\n  {expected}"
        );
    }
}

#[test]
fn the_fork_changes_only_what_it_must() {
    // The converse: anything in the fork that is not upstream's has to belong
    // to one of the changes ADR-0008 allows, and each is marked so a reader can
    // see exactly what is amon's.
    //
    // A marker covers the lines beneath it up to the next blank line, so that
    // marking one addition does not quietly bless the rest of the file.
    let upstream: Vec<&str> = UPSTREAM.lines().map(str::trim).collect();

    let mut inside_amon_block = false;
    for line in FORK.lines().map(str::trim) {
        if line.is_empty() {
            inside_amon_block = false;
            continue;
        }
        if line.contains("// amon:") {
            inside_amon_block = true;
            continue;
        }
        if line.starts_with("//") || upstream.contains(&line) {
            continue;
        }
        assert!(
            inside_amon_block,
            "line is neither upstream's nor part of a marked amon change:\n  {line}"
        );
    }
}
