//! Stands in for herdr's `crate::logging`.

/// Structured record of an install or uninstall attempt.
pub fn integration_action(action: &'static str, target: &'static str, outcome: &'static str) {
    tracing::info!(
        event = "integration.action",
        subsystem = "integration",
        outcome,
        action,
        target,
    );
}
