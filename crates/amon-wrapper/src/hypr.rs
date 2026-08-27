//! Which window the agent is running in — the wrapper's thin adapter over
//! `amon_hypr`, which owns the ancestry walk and the event following. The
//! wrapper's own contribution is knowing whose window to look for (its own)
//! and where reports go (the observer's signal channel).

use std::sync::mpsc::Sender;

use crate::observer::Signal;

/// Starts following this wrapper's window, if there is one to follow.
///
/// Returns without doing anything when the compositor is absent or the window
/// cannot be identified; the thread it spawns lives as long as the process.
pub fn spawn(signals: Sender<Signal>) {
    std::thread::spawn(move || {
        let Some(directory) = amon_hypr::socket_dir() else {
            return;
        };
        let Ok(events) = amon_hypr::connect_events(&directory) else {
            return;
        };
        let pid = std::process::id();
        let Some(window) = amon_hypr::resolve_when_mapped(&directory, pid) else {
            return;
        };

        let _ = signals.send(Signal::Window {
            window: Some(window.address.clone()),
            workspace: window.workspace.clone(),
        });

        amon_hypr::follow(&directory, events, pid, window, |window| {
            let _ = signals.send(match window {
                Some(window) => Signal::Window {
                    window: Some(window.address),
                    workspace: window.workspace,
                },
                None => Signal::Window {
                    window: None,
                    workspace: None,
                },
            });
        });
    });
}
