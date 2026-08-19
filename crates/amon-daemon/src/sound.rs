//! Making a noise when an agent starts or stops needing a human.
//!
//! The daemon plays these rather than the wrappers (ADR-0012): it sees every
//! transition, holds the configuration already, and one process making one
//! sound beats several racing to make the same one.
//!
//! Off unless asked for. A tool that starts making noise on upgrade is a tool
//! people turn off entirely.

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use amon_protocol::{AgentState, SoundConfig};

/// herdr's own sounds, bundled so `enabled = true` is enough on a fresh
/// machine. Copied by hand rather than vendored — see the README beside them.
static DONE: &[u8] = include_bytes!("../assets/sounds/done.mp3");
static BLOCKED: &[u8] = include_bytes!("../assets/sounds/request.mp3");

/// The players worth trying, most likely first. Linux only: amon's platform is
/// Omarchy, and herdr's macOS and Windows branches would be code carried for
/// nobody. Anywhere else reads as "no player" and stays quiet.
const PLAYERS: [&str; 5] = ["paplay", "pw-play", "ffplay", "mpg123", "mpv"];

/// Extra arguments a player needs to be usable as a notification: not to take
/// over the terminal, and to exit when the file ends.
fn arguments(player: &str, file: &str) -> Vec<String> {
    match player {
        "ffplay" => vec![
            "-nodisp".into(),
            "-autoexit".into(),
            "-loglevel".into(),
            "quiet".into(),
            file.into(),
        ],
        "mpv" => vec!["--no-video".into(), "--really-quiet".into(), file.into()],
        "mpg123" => vec!["-q".into(), file.into()],
        _ => vec![file.into()],
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sound {
    /// An agent finished without being watched.
    Done,
    /// An agent started waiting for a human.
    Blocked,
}

/// Which sound a transition is worth, if any.
///
/// The crossing matters, not the destination: an agent already blocked that
/// reports blocked again has not started needing anyone, and re-reporting an
/// unseen idle is not a second finish.
///
/// `Done` is deliberately narrower than herdr's, which rings on every entry to
/// idle. Amon knows whether the agent was being watched — Seen is exactly that
/// (ADR-0007) — and a notification for something you are already looking at is
/// noise. So the sound is for finishing *unnoticed*.
pub fn crossing(was: (AgentState, Option<bool>), now: (AgentState, Option<bool>)) -> Option<Sound> {
    let finished = |state, seen| state == AgentState::Idle && seen == Some(false);
    if now.0 == AgentState::Blocked && was.0 != AgentState::Blocked {
        return Some(Sound::Blocked);
    }
    if finished(now.0, now.1) && !finished(was.0, was.1) {
        return Some(Sound::Done);
    }
    None
}

/// Plays a sound, or quietly does not.
///
/// Never blocks the caller and never reports failure: a machine with no audio
/// player is not misconfigured, it is a machine that does not do sound. The
/// registry has already released its lock by the time this runs.
pub fn play(sound: Sound, config: &SoundConfig) {
    if !config.enabled {
        return;
    }
    let chosen = match sound {
        Sound::Done => config.done.clone(),
        Sound::Blocked => config.blocked.clone(),
    };
    std::thread::spawn(move || {
        let Some(file) = resolve(sound, chosen) else {
            return;
        };
        for player in PLAYERS {
            let spawned = Command::new(player)
                .args(arguments(player, &file.path))
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn();
            if let Ok(mut child) = spawned {
                let _ = child.wait();
                return;
            }
        }
    });
}

/// The file to hand a player: the user's if they named one, otherwise the
/// bundled bytes written somewhere a player can open them.
fn resolve(sound: Sound, configured: Option<String>) -> Option<Playable> {
    if let Some(configured) = configured {
        let path = expand(&configured)?;
        return path.exists().then(|| Playable {
            path: path.to_string_lossy().into_owned(),
            temporary: false,
        });
    }
    let bytes = match sound {
        Sound::Done => DONE,
        Sound::Blocked => BLOCKED,
    };
    let path = std::env::temp_dir().join(format!(
        "amon-{}-{}.mp3",
        match sound {
            Sound::Done => "done",
            Sound::Blocked => "blocked",
        },
        std::process::id()
    ));
    // Rewritten every time rather than cached: it costs a few tens of
    // kilobytes and means a half-written file from a killed daemon cannot
    // become a permanently silent notification.
    let mut file = std::fs::File::create(&path).ok()?;
    file.write_all(bytes).ok()?;
    Some(Playable {
        path: path.to_string_lossy().into_owned(),
        temporary: true,
    })
}

/// A path a player can be pointed at.
struct Playable {
    path: String,
    /// Bundled bytes we wrote out, and therefore ours to clean up.
    temporary: bool,
}

impl Drop for Playable {
    fn drop(&mut self) {
        if self.temporary {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

/// Resolves `~` and makes a relative path relative to the config file's own
/// directory, the way herdr's does — a config that names `chime.mp3` means the
/// one sitting beside it.
fn expand(path: &str) -> Option<PathBuf> {
    if let Some(rest) = path.strip_prefix("~/") {
        let home = std::env::var_os("HOME")?;
        return Some(PathBuf::from(home).join(rest));
    }
    let path = PathBuf::from(path);
    if path.is_absolute() {
        return Some(path);
    }
    Some(amon_detect::config_dir().join(path))
}

#[cfg(test)]
mod tests {
    use super::*;

    const UNSEEN: Option<bool> = Some(false);
    const SEEN: Option<bool> = Some(true);

    #[test]
    fn starting_to_wait_for_a_human_is_worth_a_noise() {
        assert_eq!(
            crossing((AgentState::Working, SEEN), (AgentState::Blocked, UNSEEN)),
            Some(Sound::Blocked)
        );
    }

    #[test]
    fn staying_blocked_is_not_a_second_request() {
        // A wrapper re-reports the state it is already in whenever anything
        // else about the entry changes. Ringing for that would turn one
        // question into a stream of them.
        assert_eq!(
            crossing((AgentState::Blocked, UNSEEN), (AgentState::Blocked, UNSEEN)),
            None
        );
    }

    #[test]
    fn finishing_unwatched_is_worth_a_noise() {
        assert_eq!(
            crossing((AgentState::Working, SEEN), (AgentState::Idle, UNSEEN)),
            Some(Sound::Done)
        );
    }

    #[test]
    fn finishing_while_you_watch_is_not() {
        // The divergence from herdr, which rings on every entry to idle. Seen
        // is exactly "you were looking when it happened" (ADR-0007), and a
        // notification for something already in front of you is noise.
        assert_eq!(
            crossing((AgentState::Working, SEEN), (AgentState::Idle, SEEN)),
            None
        );
    }

    #[test]
    fn an_agent_that_stays_finished_rings_once() {
        assert_eq!(
            crossing((AgentState::Idle, UNSEEN), (AgentState::Idle, UNSEEN)),
            None
        );
    }

    #[test]
    fn looking_at_a_finished_agent_is_silent() {
        // Seen flipping true is the user arriving, not the agent doing
        // anything. Nothing crossed.
        assert_eq!(
            crossing((AgentState::Idle, UNSEEN), (AgentState::Idle, SEEN)),
            None
        );
    }

    #[test]
    fn a_silenced_config_spawns_nothing() {
        // The whole guard: `enabled` defaults false, so an upgrade never
        // starts making noise at somebody.
        assert!(!SoundConfig::default().enabled);
    }
}
