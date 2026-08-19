//! Reading `~/.config/amon/config.toml`, and noticing when it changes.
//!
//! The daemon is the only thing that reads it (ADR-0012): subscribers are told
//! what it says over the socket they already hold, so nothing else needs a TOML
//! parser or needs to know where the file lives.
//!
//! Polled rather than watched. A second of latency on a file edited by hand a
//! few times in its life is not worth a dependency, and statting the path each
//! time means an editor that saves by rename — which is most of them — is
//! picked up like any other change.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use amon_protocol::Config;

/// How often the file is stat-ed.
const POLL_INTERVAL: Duration = Duration::from_secs(1);

/// Where the file lives. `~/.config/amon/config.toml`, or `amon-dev` under a
/// debug build, which is the same split the state and runtime directories make.
pub fn path() -> Option<PathBuf> {
    Some(amon_detect::config_dir().join("config.toml"))
}

/// What the file says, and what went wrong reading it.
///
/// A parse failure keeps the last good answer rather than reverting to
/// defaults: the bar should not change appearance because someone saved a file
/// mid-edit, and the error has somewhere to be reported.
#[derive(Debug, Clone, Default)]
pub struct Loaded {
    pub config: Config,
    /// `None` when the file parsed, or is simply absent.
    pub error: Option<String>,
}

/// Reads the file once. An absent file is not an error — it is the default
/// configuration, which is what most machines will run.
pub fn read() -> Loaded {
    let Some(path) = path() else {
        return Loaded::default();
    };
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Loaded::default(),
        Err(error) => {
            return Loaded {
                config: Config::default(),
                error: Some(format!("{}: {error}", path.display())),
            }
        }
    };
    match toml::from_str::<Config>(&text) {
        Ok(config) => Loaded {
            config,
            error: None,
        },
        Err(error) => Loaded {
            config: Config::default(),
            error: Some(format!("{}: {error}", path.display())),
        },
    }
}

/// The daemon's live view of the file.
#[derive(Clone, Default)]
pub struct Watcher {
    inner: Arc<Mutex<Loaded>>,
}

impl Watcher {
    /// Reads the file and starts polling it, calling `on_change` with each new
    /// configuration — including the first.
    ///
    /// The callback runs on the polling thread and is expected to be cheap:
    /// broadcasting to subscribers is a channel send per subscriber.
    pub fn start(on_change: impl Fn(&Config) + Send + 'static) -> Self {
        let watcher = Self::default();
        let mirror = watcher.clone();
        let first = read();
        on_change(&first.config);
        *watcher.inner.lock().expect("config lock") = first;

        std::thread::spawn(move || {
            let mut stamp = mtime();
            loop {
                std::thread::sleep(POLL_INTERVAL);
                let now = mtime();
                if now == stamp {
                    continue;
                }
                stamp = now;
                let next = read();
                {
                    let mut held = mirror.inner.lock().expect("config lock");
                    // A parse failure keeps the previous configuration, so a
                    // half-written file does not blank the bar on save.
                    if next.error.is_some() {
                        held.error = next.error;
                        continue;
                    }
                    if next.config == held.config {
                        held.error = None;
                        continue;
                    }
                    *held = next;
                }
                let held = mirror.inner.lock().expect("config lock").config.clone();
                on_change(&held);
            }
        });

        watcher
    }

    pub fn current(&self) -> Config {
        self.inner.lock().expect("config lock").config.clone()
    }
}

/// The file's modification time, or `None` when it is not there. Both are
/// stable answers, so appearing and disappearing both read as a change.
fn mtime() -> Option<SystemTime> {
    let path = path()?;
    std::fs::metadata(path).ok()?.modified().ok()
}
