//! Making a noise when an agent starts or stops needing a human.
//!
//! The daemon plays these rather than the wrappers (ADR-0012): it sees every
//! transition, holds the configuration already, and one process making one
//! sound beats several racing to make the same one.
//!
//! On by default, and rare by construction: an agent finishing while you were
//! not looking, or starting to wait for you. A notification nobody knows exists
//! notifies nobody, which is why it is not opt-in — the config file `amon
//! setup` writes shows how to turn it off.

use std::collections::HashMap;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use amon_protocol::{AgentState, SoundConfig};

/// herdr's own sounds, bundled so `enabled = true` is enough on a fresh
/// machine. Copied by hand rather than vendored — see the README beside them.
static DONE: &[u8] = include_bytes!("../assets/sounds/done.mp3");
static BLOCKED: &[u8] = include_bytes!("../assets/sounds/request.mp3");

/// The one player: pw-play, PipeWire's own, pinned by Omarchy's base
/// packages. herdr's fallback chain (paplay, ffplay, mpg123, mpv) was
/// carried for machines amon does not target; a machine without pw-play
/// reads as "no player" and stays quiet, the same stance as everywhere else.
const PLAYER: &str = "pw-play";

/// The stream declares itself a notification. That is what lets the ducking
/// drop-in (`amon setup --duck`) route it onto the Notification bus, above
/// the music; without the drop-in the tag is inert. A test holds this role
/// to the one the drop-in ranks.
const ROLE: &str = r#"{ media.role = "Notification" }"#;

fn arguments(file: &str) -> Vec<String> {
    vec!["-P".into(), ROLE.into(), file.into()]
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
    if now.0 == AgentState::Blocked && was.0 != AgentState::Blocked {
        return Some(Sound::Blocked);
    }
    // Finishing means arriving at idle *from doing something*. herdr draws the
    // same line, and it matters: an agent whose state was merely unrecognized
    // has not just completed anything, and saying so would ring for every
    // wrapper that settles into a prompt at launch.
    let came_from_work = matches!(was.0, AgentState::Working | AgentState::Blocked);
    if now.0 == AgentState::Idle && now.1 == Some(false) && came_from_work {
        return Some(Sound::Done);
    }
    None
}

/// How long a crossing has to survive before it is worth a noise.
///
/// Detection passes through intermediate states on its way to a settled one: a
/// prompt appearing reads as idle for a moment before the rule that recognises
/// it as a question matches. Playing immediately turns one event into two — a
/// finish, then a request — which is what this delay exists to prevent. herdr's
/// default is the same second, for the same reason.
const SETTLE: Duration = Duration::from_secs(1);

/// One agent's pending noise, and whether it is still the current one.
///
/// Keyed by agent rather than globally: two agents settling at once are two
/// separate events, and one should not silence the other.
#[derive(Clone, Default)]
pub struct Notifier {
    /// Agent id to the generation of its most recent crossing. A pending sound
    /// carries the generation it was scheduled with and gives up if it no
    /// longer matches, which is how a superseded state cancels its own noise
    /// without anything having to find and stop a thread.
    generations: Arc<Mutex<HashMap<String, u64>>>,
}

impl Notifier {
    /// Records what an agent just did.
    ///
    /// `None` is not "nothing happened" — it cancels whatever that agent had
    /// pending. A change that is worth no sound is still evidence that the
    /// previous one had not settled.
    pub fn record(&self, agent: &str, sound: Option<Sound>, config: &SoundConfig) {
        let generation = {
            let mut generations = self.generations.lock().expect("sound lock");
            let generation = generations.entry(agent.to_string()).or_insert(0);
            *generation += 1;
            *generation
        };
        let Some(sound) = sound else {
            return;
        };
        if !config.enabled {
            return;
        }
        let config = config.clone();
        let agent = agent.to_string();
        let generations = self.generations.clone();
        std::thread::spawn(move || {
            std::thread::sleep(SETTLE);
            let current = generations
                .lock()
                .expect("sound lock")
                .get(&agent)
                .copied()
                .unwrap_or(0);
            // Superseded: the agent moved on before this had settled, and the
            // later crossing is the one worth hearing.
            if current != generation {
                return;
            }
            play(sound, &config);
        });
    }

    /// Forgets an agent that has gone, so the map does not grow across a long
    /// day of sessions.
    pub fn forget(&self, agent: &str) {
        self.generations.lock().expect("sound lock").remove(agent);
    }
}

/// Plays a sound, or quietly does not.
///
/// Never blocks the caller and never reports failure: a machine with no audio
/// player is not misconfigured, it is a machine that does not do sound.
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
        let spawned = Command::new(PLAYER)
            .args(arguments(&file))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
        if let Ok(mut child) = spawned {
            let _ = child.wait();
        }
    });
}

/// The file to hand a player: the user's if they named one, otherwise the
/// bundled sound as a real file in amon's share directory.
///
/// The bundled sounds are bytes in the binary, so they have to land on disk
/// once — and once is the point. An earlier design wrote a temp file per
/// play and deleted it after: two overlapping plays then shared one path,
/// and the second truncated the file under the first player's read, which
/// is a sound cut off. Here the path is stable, an intact file is never
/// touched, and repair goes through a rename — a player mid-read keeps its
/// bytes while the path moves on.
fn resolve(sound: Sound, configured: Option<String>) -> Option<String> {
    if let Some(configured) = configured {
        let path = expand(&configured)?;
        return path.exists().then(|| path.to_string_lossy().into_owned());
    }
    let (name, bytes) = match sound {
        Sound::Done => ("done.mp3", DONE),
        Sound::Blocked => ("request.mp3", BLOCKED),
    };
    let dir = sounds_dir()?;
    let path = dir.join(name);
    // Healed whenever the size disagrees with the binary's copy: missing,
    // torn, or left over from another release. Nothing else ever writes.
    let current = std::fs::metadata(&path).map(|meta| meta.len()).ok();
    if current != Some(bytes.len() as u64) {
        std::fs::create_dir_all(&dir).ok()?;
        let staging = dir.join(format!(".{name}.{}", std::process::id()));
        let mut file = std::fs::File::create(&staging).ok()?;
        file.write_all(bytes).ok()?;
        std::fs::rename(&staging, &path).ok()?;
    }
    Some(path.to_string_lossy().into_owned())
}

/// Where the bundled sounds live as files, beside the licenses the installer
/// puts in the same share directory. Per-build like every other amon
/// directory, so a dev daemon cannot write into a release install.
fn sounds_dir() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_DATA_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME")
                .filter(|value| !value.is_empty())
                .map(|home| PathBuf::from(home).join(".local/share"))
        })?;
    Some(
        base.join(amon_protocol::paths::app_dir_name())
            .join("sounds"),
    )
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
    fn settling_into_a_prompt_is_not_finishing() {
        // An agent whose state was never recognised has not just completed
        // anything. Without this every wrapper that starts up and lands on a
        // prompt would announce itself as finished.
        assert_eq!(
            crossing((AgentState::Unknown, None), (AgentState::Idle, UNSEEN)),
            None
        );
    }

    #[test]
    fn a_crossing_superseded_within_the_settle_makes_no_noise() {
        // The bug this exists for: detection reads a new prompt as idle for a
        // moment before recognising it as a question, so playing immediately
        // rang "finished" and then "needs you" for one event.
        let notifier = Notifier::default();
        let config = SoundConfig {
            enabled: true,
            ..SoundConfig::default()
        };
        notifier.record("a1", Some(Sound::Done), &config);
        notifier.record("a1", Some(Sound::Blocked), &config);

        // Whatever is still pending, only one generation can match, so at most
        // one of the two can ever play.
        let generations = notifier.generations.lock().expect("lock");
        assert_eq!(generations.get("a1"), Some(&2));
    }

    #[test]
    fn a_change_worth_no_sound_still_cancels_what_was_pending() {
        // Recording None is not "nothing happened": it is evidence the earlier
        // crossing had not settled, so it must invalidate it.
        let notifier = Notifier::default();
        let config = SoundConfig {
            enabled: true,
            ..SoundConfig::default()
        };
        notifier.record("a1", Some(Sound::Done), &config);
        notifier.record("a1", None, &config);

        assert_eq!(
            notifier.generations.lock().expect("lock").get("a1"),
            Some(&2),
            "the pending sound's generation is stale, so it gives up"
        );
    }

    #[test]
    fn two_agents_do_not_silence_each_other() {
        let notifier = Notifier::default();
        let config = SoundConfig {
            enabled: true,
            ..SoundConfig::default()
        };
        notifier.record("a1", Some(Sound::Done), &config);
        notifier.record("a2", Some(Sound::Blocked), &config);

        let generations = notifier.generations.lock().expect("lock");
        assert_eq!(generations.get("a1"), Some(&1));
        assert_eq!(generations.get("a2"), Some(&1));
    }

    #[test]
    fn a_departed_agent_is_forgotten() {
        let notifier = Notifier::default();
        notifier.record("a1", None, &SoundConfig::default());
        notifier.forget("a1");
        assert!(notifier.generations.lock().expect("lock").is_empty());
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
    fn bundled_sounds_are_real_files_that_heal() {
        // Sets an env var, which is why this suite runs under nextest: one
        // process per test, nothing to race.
        let dir = std::env::temp_dir().join(format!("amon-sound-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::env::set_var("XDG_DATA_HOME", &dir);

        let path = resolve(Sound::Done, None).expect("a playable path");
        assert_eq!(
            std::fs::read(&path).expect("the file exists").as_slice(),
            DONE,
            "the bundled bytes, installed where a player can open them"
        );

        // Damaged — a torn write, a stray truncation — the next play repairs
        // it rather than handing the player a stub forever.
        std::fs::write(&path, b"stub").unwrap();
        let again = resolve(Sound::Done, None).expect("still playable");
        assert_eq!(again, path, "the path is stable across plays");
        assert_eq!(
            std::fs::read(&path).unwrap().as_slice(),
            DONE,
            "and the content is whole again"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_intact_sound_file_is_not_rewritten() {
        // Rewriting on every play is the temp-file design this replaced: a
        // player mid-read of a file being truncated under it is a sound cut
        // off. An intact file must be left exactly as it is.
        let dir = std::env::temp_dir().join(format!("amon-sound-keep-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::env::set_var("XDG_DATA_HOME", &dir);

        let path = resolve(Sound::Blocked, None).expect("a playable path");
        let before = std::fs::metadata(&path).unwrap().modified().unwrap();
        std::thread::sleep(Duration::from_millis(20));
        resolve(Sound::Blocked, None).expect("still playable");
        let after = std::fs::metadata(&path).unwrap().modified().unwrap();
        assert_eq!(before, after, "a second play writes nothing");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn playback_declares_the_notification_role() {
        // The tag is what the ducking drop-in routes by; losing it would not
        // fail anything visibly — the sound would simply stop ducking music.
        let args = arguments("chime.mp3");
        assert_eq!(args[0], "-P");
        assert!(args[1].contains("media.role"), "{args:?}");
        assert_eq!(args.last().map(String::as_str), Some("chime.mp3"));
    }

    #[test]
    fn sound_is_on_unless_turned_off() {
        // Deliberately not opt-in: these fire only when an agent finished
        // unwatched or started waiting, and a notification nobody knows exists
        // notifies nobody.
        assert!(SoundConfig::default().enabled);
    }

    #[test]
    fn turning_it_off_stops_everything() {
        let silent = SoundConfig {
            enabled: false,
            ..SoundConfig::default()
        };
        let notifier = Notifier::default();
        notifier.record("a1", Some(Sound::Done), &silent);
        // Recorded — a crossing still supersedes what came before — but with
        // nothing scheduled behind it.
        assert_eq!(
            notifier.generations.lock().expect("lock").get("a1"),
            Some(&1)
        );
    }
}
