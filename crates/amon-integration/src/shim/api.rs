//! Stands in for herdr's `crate::api`.
//!
//! The vendored installer names its targets through this type and reads the
//! socket-path environment variable when preparing an agent's environment.

pub const SOCKET_PATH_ENV_VAR: &str = amon_protocol::env::SOCKET_PATH;

/// herdr points spawned agents at its daemon socket. amon points them at the
/// wrapper that spawned them (ADR-0001), and only the wrapper knows that path,
/// so it sets the variable itself. This exists for the vendored
/// `apply_pane_base_env`, which amon does not use.
pub fn socket_path() -> std::path::PathBuf {
    std::env::var_os(SOCKET_PATH_ENV_VAR)
        .map(std::path::PathBuf::from)
        .unwrap_or_default()
}

pub mod schema {
    use serde::{Deserialize, Serialize};

    /// Every agent amon can install an integration for. Kept in herdr's order
    /// so the vendored match arms stay exhaustive and reviewable against
    /// upstream.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(rename_all = "snake_case")]
    pub enum IntegrationTarget {
        Pi,
        Omp,
        Claude,
        Codex,
        Copilot,
        Devin,
        Droid,
        Kimi,
        Opencode,
        Kilo,
        Hermes,
        Qodercli,
        Cursor,
        Mastracode,
        Grok,
    }

    impl IntegrationTarget {
        pub const ALL: [Self; 15] = [
            Self::Pi,
            Self::Omp,
            Self::Claude,
            Self::Codex,
            Self::Copilot,
            Self::Devin,
            Self::Droid,
            Self::Kimi,
            Self::Opencode,
            Self::Kilo,
            Self::Hermes,
            Self::Qodercli,
            Self::Cursor,
            Self::Mastracode,
            Self::Grok,
        ];
    }
}
