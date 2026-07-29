//! Stands in for herdr's `crate::config`.
//!
//! Only the directory lookups the vendored detect engine actually calls,
//! pointed at gaze's own directories so the two tools never share state.

use std::path::PathBuf;

pub fn app_dir_name() -> &'static str {
    if cfg!(debug_assertions) {
        "gaze-dev"
    } else {
        "gaze"
    }
}

pub fn config_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("XDG_CONFIG_HOME") {
        return PathBuf::from(dir).join(app_dir_name());
    }
    home_relative(".config")
}

pub fn state_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("XDG_STATE_HOME") {
        return PathBuf::from(dir).join(app_dir_name());
    }
    home_relative(".local/state")
}

fn home_relative(suffix: &str) -> PathBuf {
    match std::env::var("HOME") {
        Ok(home) => PathBuf::from(home).join(suffix).join(app_dir_name()),
        Err(_) => std::env::temp_dir().join(app_dir_name()),
    }
}

/// Serializes tests that mutate directory environment variables. The vendored
/// manifest tests take this lock.
#[cfg(test)]
pub fn test_config_env_lock() -> &'static std::sync::Mutex<()> {
    static LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
    LOCK.get_or_init(|| std::sync::Mutex::new(()))
}
