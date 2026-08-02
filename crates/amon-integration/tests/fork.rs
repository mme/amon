//! The Omarchy widget is a fork, and forks drift (ADR-0008).
//!
//! The rule the ADR states is that the only difference from upstream is the
//! dot layer. That is only enforceable against a copy of upstream, so one is
//! checked in beside this test — not installed, not compiled in, just what the
//! fork was derived from.
//!
//! Re-syncing means replacing both: the fixture with the new upstream file,
//! and the shipped widget with upstream plus the three marked additions.

/// Omarchy Quattro's `shell/plugins/bar/widgets/Workspaces.qml`, taken
/// 2026-08-02.
const UPSTREAM: &str = include_str!("fixtures/omarchy-Workspaces.qml");

/// What amon ships, upstream plus the dot.
const FORK: &str = include_str!("../src/assets/omarchy/Workspaces.qml");

/// The one upstream line the fork is allowed to change: a plugin has to name
/// itself, and two plugins cannot share an id.
const RENAMED: &str = "moduleName:";

#[test]
fn the_fork_only_adds_to_upstream() {
    // Every upstream line must still be there, in order. Insertions are the
    // whole point; a changed or deleted line is drift, and turns the next
    // re-sync from a re-application into a merge.
    let mut fork = FORK.lines().map(str::trim);

    for expected in UPSTREAM.lines().map(str::trim) {
        if expected.is_empty() || expected.starts_with(RENAMED) {
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
    // The converse: anything in the fork that is not upstream has to be one of
    // the additions ADR-0008 allows, and they are marked so a reader can see
    // exactly what is amon's.
    let upstream: Vec<&str> = UPSTREAM.lines().map(str::trim).collect();

    let mut inside_amon_block = false;
    for line in FORK.lines().map(str::trim) {
        if line.contains("// amon:") {
            inside_amon_block = true;
            continue;
        }
        if line.is_empty() || line.starts_with("//") || upstream.contains(&line) {
            continue;
        }
        assert!(
            inside_amon_block,
            "line is neither upstream's nor part of a marked amon addition:\n  {line}"
        );
    }
}
