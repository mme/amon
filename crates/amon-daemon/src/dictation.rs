//! Ducking music while dictation records.
//!
//! Omarchy's dictation is voxtype, and voxtype's daemon writes its state to a
//! file on every transition — the same seam the bar's indicator reads. While
//! that state says the microphone is live, this module holds open one silent
//! stream tagged `media.role=Notification`; the ducking drop-in (`amon setup
//! --duck`) then dips the Multimedia bus for exactly the stream's lifetime,
//! the same way it does under a chime. No volume is ever set imperatively, so
//! there is nothing to restore and nothing a crash can leave quiet: the
//! silence is fed through a pipe this daemon holds, and a daemon that dies
//! takes the pipe — and the duck — with it.

use std::io::Write;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// How often the state file is read. Dictation starts with a keypress and the
/// duck should land while the first word is still being spoken; at 100ms the
/// gap is below what a listener attributes to the press at all.
const POLL: Duration = Duration::from_millis(100);

/// How long a duck stream that died on its own blocks the next attempt.
/// Long enough to turn a broken player into one fork a second instead of
/// ten; short enough that a transient PipeWire hiccup barely shows.
const RESPAWN_BACKOFF: Duration = Duration::from_secs(1);

/// Whether a voxtype state means the microphone is live.
///
/// `recording` and `streaming` are the capturing states; `transcribing` is
/// not — the mic is closed, the music may return while whisper works. Both
/// consumers hang off this one reading: the duck holds its stream while the
/// mic is live, and the dictate button's press means "stop" exactly when it
/// is.
fn microphone_live(state: &str) -> bool {
    matches!(state.trim(), "recording" | "streaming")
}

/// The predicate against the file on disk, for the edges of the dictate
/// button. Absent — no voxtype, reporting disabled — reads as not live,
/// which makes a press a start and a release a no-op: the safe defaults.
pub fn microphone_live_now() -> bool {
    state_path()
        .and_then(|path| std::fs::read_to_string(path).ok())
        .is_some_and(|state| microphone_live(&state))
}

/// Where voxtype's daemon writes its state: `$XDG_RUNTIME_DIR/voxtype/state`,
/// the default that Omarchy's shipped config (`state_file = "auto"`) resolves
/// to. Absent — no runtime dir, no voxtype, state reporting disabled — reads
/// as idle, and the module stays quietly inert.
pub(crate) fn state_path() -> Option<PathBuf> {
    let dir = std::env::var_os("XDG_RUNTIME_DIR").filter(|value| !value.is_empty())?;
    Some(PathBuf::from(dir).join("voxtype/state"))
}

/// Starts the watcher thread, which runs for the daemon's life.
///
/// Nothing to stop at shutdown: the held player reads its silence from a pipe
/// this process owns, so however the daemon ends, the stream ends with it and
/// the drop-in restores the volume — the crash-safety ADR-0014 built the role
/// bus for.
pub fn watch(config: crate::config::Watcher) {
    std::thread::spawn(move || {
        let mut duck = Duck::new(player_command(), RESPAWN_BACKOFF);
        loop {
            let wanted = config.current().sound.duck_while_dictating && microphone_live_now();
            duck.apply(wanted);
            std::thread::sleep(POLL);
        }
    });
}

/// The player `sound.rs` standardizes on, on the role the drop-in ducks
/// under, fed raw silence on stdin: 8000Hz mono s16, the smallest format
/// PipeWire resamples without comment — the stream exists to be a stream,
/// nobody hears it.
fn player_command() -> Vec<String> {
    [
        crate::sound::PLAYER,
        "-P",
        crate::sound::ROLE,
        "--raw",
        "--rate",
        "8000",
        "--channels",
        "1",
        "--format",
        "s16",
        "-",
    ]
    .map(String::from)
    .to_vec()
}

// ---------------------------------------------------------------------------
// Hold-to-talk: what the dictate button's edges mean.
// ---------------------------------------------------------------------------

/// What a press or release decided to do about recording.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DictateCommand {
    Start,
    Stop,
    /// Stop, and have voxtype press Enter after the text lands.
    StopSubmit,
}

impl DictateCommand {
    /// The exact CLI, verified against voxtype 0.7.5: `stop` cannot carry
    /// `--auto-submit`, so a submitting stop goes through `toggle`, which
    /// can — the caller guards that toggle with a recording check so it can
    /// only ever stop.
    pub fn line(self) -> &'static str {
        match self {
            DictateCommand::Start => "voxtype record start",
            DictateCommand::Stop => "voxtype record stop",
            DictateCommand::StopSubmit => "voxtype record toggle --auto-submit",
        }
    }
}

/// The press length that separates a tap from a hold. QMK's tapping term —
/// the same judgment about the same fingers — defaults to 200ms; a little
/// above that keeps a deliberate tap from reading as a hold, and nothing
/// here waits on it: only the release consults it.
const DEFAULT_HOLD: Duration = Duration::from_millis(250);

/// The threshold the config asks for, or the tuned default. The protocol
/// deliberately carries options, not defaults (its own doctrine): the
/// consumer owns the numbers.
pub fn hold_of(config: &amon_protocol::Micro2Config) -> Duration {
    config
        .dictation
        .hold_ms
        .map_or(DEFAULT_HOLD, Duration::from_millis)
}

/// Whether a hold's end should press Enter. On unless turned off: "dictate,
/// let go, and it sends" is the point of holding.
pub fn auto_submit_of(config: &amon_protocol::Micro2Config) -> bool {
    config.dictation.auto_submit.unwrap_or(true)
}

/// The dictate button's state between edges. Tap and hold both start
/// recording on the press — nothing waits on the threshold — and the time
/// until release decides what the release means: nothing (a tap; recording
/// runs until the next tap stops it) or stop-and-submit (a hold ends).
#[derive(Debug, Default)]
pub struct HoldToTalk {
    /// When this button last started a recording, and is still down.
    pressed_at: Option<Instant>,
}

impl HoldToTalk {
    /// A press: stop when recording (the second tap of a toggle), start
    /// otherwise.
    pub fn on_press(&mut self, now: Instant, recording: bool) -> DictateCommand {
        if recording {
            self.pressed_at = None;
            DictateCommand::Stop
        } else {
            self.pressed_at = Some(now);
            DictateCommand::Start
        }
    }

    /// A release: `None` for a tap, the configured stop for a hold. Only a
    /// press that started a recording arms this — the release of a
    /// toggle-stopping tap says nothing.
    pub fn on_release(
        &mut self,
        now: Instant,
        hold: Duration,
        auto_submit: bool,
    ) -> Option<DictateCommand> {
        let pressed_at = self.pressed_at.take()?;
        if now.duration_since(pressed_at) < hold {
            return None;
        }
        Some(if auto_submit {
            DictateCommand::StopSubmit
        } else {
            DictateCommand::Stop
        })
    }

    /// The stop for a release that can never arrive — the device unplugged,
    /// the module disabled, the daemon going down, all mid-hold. `None`
    /// when no press is armed: a tap's recording was deliberately left
    /// running and is voxtype's to finish.
    fn abort(&mut self) -> Option<DictateCommand> {
        self.pressed_at.take().map(|_| DictateCommand::Stop)
    }
}

/// The device loop drops its `HoldToTalk` on every way out; a held button
/// must not leave the microphone live until voxtype's duration cap. Plain
/// stop, never submit — nobody chose to end this recording. (`record stop`
/// is idempotent, so a recording that already ended costs a no-op signal.)
impl Drop for HoldToTalk {
    fn drop(&mut self) {
        if self.abort().is_some() {
            crate::devices::actions::exec(DictateCommand::Stop.line());
        }
    }
}

/// The one silent stream, and the process holding it.
///
/// The player command is injected so the lifecycle is testable without an
/// audio stack; the daemon runs it with pw-play, the player `sound.rs`
/// already standardizes on.
struct Duck {
    player: Vec<String>,
    child: Option<Child>,
    /// How long a died-on-its-own stream blocks the next spawn — the brake
    /// that keeps a broken player from becoming ten forks a second for a
    /// whole recording. Injected so the tests need no clock.
    respawn_backoff: Duration,
    last_spawn: Option<Instant>,
}

impl Duck {
    fn new(player: Vec<String>, respawn_backoff: Duration) -> Self {
        Self {
            player,
            child: None,
            respawn_backoff,
            last_spawn: None,
        }
    }

    /// Brings the stream in line with whether ducking is wanted right now.
    /// Idempotent by design: the poll loop calls this every tick — which is
    /// also what respawns a player that fell over mid-recording.
    fn apply(&mut self, want: bool) {
        if let Some(child) = &mut self.child {
            let alive = matches!(child.try_wait(), Ok(None));
            match (want, alive) {
                (true, true) => return,
                (true, false) => self.child = None,
                (false, _) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    self.child = None;
                    // A killed stream was a working stream: the next
                    // recording deserves its duck immediately.
                    self.last_spawn = None;
                    return;
                }
            }
        }
        if want && !self.backing_off() {
            self.last_spawn = Some(Instant::now());
            self.child = spawn(&self.player);
        }
    }

    /// Whether the last spawn is still too fresh to follow up on. Only a
    /// stream that *died* gets here — a killed one cleared its slot on the
    /// way out — so recent means the player is failing, not working.
    fn backing_off(&self) -> bool {
        self.last_spawn
            .is_some_and(|at| at.elapsed() < self.respawn_backoff)
    }
}

/// Starts the player and the thread feeding it silence.
///
/// The silence arrives over a pipe rather than from `/dev/zero` or a file on
/// disk, and the pipe is the point: it is this process that holds the write
/// end, so the stream — and with it the duck — cannot outlive the daemon by
/// more than the pipe buffer. The feeder thread ends itself on the first
/// failed write, which is what a killed player looks like from here.
fn spawn(player: &[String]) -> Option<Child> {
    let (command, arguments) = player.split_first()?;
    let mut child = Command::new(command)
        .args(arguments)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let mut stdin = child.stdin.take()?;
    std::thread::spawn(move || {
        // 100ms of audio per write at the rate `player_command` asks for —
        // the writes block on the pipe, so this thread mostly sleeps.
        let silence = [0u8; 1600];
        while stdin.write_all(&silence).is_ok() {}
    });
    Some(child)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn duck(command: &[&str]) -> Duck {
        Duck::new(
            command.iter().map(|part| part.to_string()).collect(),
            Duration::ZERO,
        )
    }

    fn pid_of(duck: &Duck) -> Option<u32> {
        duck.child.as_ref().map(|child| child.id())
    }

    fn gone(pid: u32) -> bool {
        // Reaped is the bar, not just killed: a zombie would accumulate
        // across a day of dictation.
        !std::path::Path::new(&format!("/proc/{pid}")).exists()
    }

    #[test]
    fn wanting_holds_one_stream_not_many() {
        let mut duck = duck(&["cat"]);
        duck.apply(true);
        let first = pid_of(&duck).expect("a stream is held");
        duck.apply(true);
        assert_eq!(pid_of(&duck), Some(first), "a held stream is not respawned");
        duck.apply(false);
    }

    #[test]
    fn releasing_kills_and_reaps_the_stream() {
        let mut duck = duck(&["cat"]);
        duck.apply(true);
        let pid = pid_of(&duck).expect("a stream is held");
        duck.apply(false);
        assert_eq!(pid_of(&duck), None);
        assert!(gone(pid));
    }

    #[test]
    fn a_freshly_dead_stream_waits_out_the_backoff() {
        // `true` exits immediately — a player that cannot hold a stream.
        // Without a pause between respawns, a recording with a broken
        // player becomes ten forks a second for its whole duration.
        let mut duck = Duck::new(
            vec!["true".into()],
            Duration::from_secs(60), // longer than the test, so: never
        );
        duck.apply(true);
        pid_of(&duck).expect("a stream is held");
        while !matches!(
            duck.child.as_mut().map(|child| child.try_wait()),
            Some(Ok(Some(_)))
        ) {
            std::thread::sleep(Duration::from_millis(5));
        }
        duck.apply(true);
        assert_eq!(
            pid_of(&duck),
            None,
            "a respawn inside the backoff window is declined"
        );
    }

    #[test]
    fn a_stream_that_died_is_respawned_while_wanted() {
        // `true` exits immediately — a player that fell over mid-recording.
        let mut duck = duck(&["true"]);
        duck.apply(true);
        let first = pid_of(&duck).expect("a stream is held");
        let exited = |duck: &mut Duck| {
            duck.child
                .as_mut()
                .is_some_and(|child| matches!(child.try_wait(), Ok(Some(_))))
        };
        for _ in 0..500 {
            if exited(&mut duck) {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(exited(&mut duck), "`true` exits promptly");
        duck.apply(true);
        let second = pid_of(&duck).expect("a replacement is held");
        assert_ne!(second, first);
        duck.apply(false);
    }

    const HOLD: Duration = Duration::from_millis(250);

    #[test]
    fn a_tap_starts_recording_and_leaves_it_running() {
        let mut button = HoldToTalk::default();
        let pressed = Instant::now();
        assert_eq!(button.on_press(pressed, false), DictateCommand::Start);
        let released = pressed + Duration::from_millis(120);
        assert_eq!(
            button.on_release(released, HOLD, true),
            None,
            "a tap's release means nothing — the recording is a toggle now"
        );
    }

    #[test]
    fn a_second_tap_stops_without_submitting() {
        let mut button = HoldToTalk::default();
        let pressed = Instant::now();
        assert_eq!(button.on_press(pressed, true), DictateCommand::Stop);
        assert_eq!(
            button.on_release(pressed + Duration::from_millis(80), HOLD, true),
            None,
            "the stopping tap's release is not a hold ending"
        );
    }

    #[test]
    fn a_hold_is_push_to_talk() {
        let mut button = HoldToTalk::default();
        let pressed = Instant::now();
        assert_eq!(button.on_press(pressed, false), DictateCommand::Start);
        assert_eq!(
            button.on_release(pressed + HOLD, HOLD, true),
            Some(DictateCommand::StopSubmit),
            "the threshold itself already counts as holding"
        );
    }

    #[test]
    fn auto_submit_off_makes_a_hold_end_plainly() {
        let mut button = HoldToTalk::default();
        let pressed = Instant::now();
        button.on_press(pressed, false);
        assert_eq!(
            button.on_release(pressed + Duration::from_secs(2), HOLD, false),
            Some(DictateCommand::Stop)
        );
    }

    #[test]
    fn an_abandoned_hold_stops_plainly() {
        // The device vanishing, the module being disabled, the daemon going
        // down — a release that can never arrive must not leave the
        // microphone live until voxtype's cap.
        let mut button = HoldToTalk::default();
        button.on_press(Instant::now(), false);
        assert_eq!(button.abort(), Some(DictateCommand::Stop));
        assert_eq!(button.abort(), None, "aborting is once");

        let mut released = HoldToTalk::default();
        let pressed = Instant::now();
        released.on_press(pressed, false);
        released.on_release(pressed + Duration::from_millis(10), HOLD, true);
        assert_eq!(
            released.abort(),
            None,
            "a tap already let go — its recording is deliberate"
        );
    }

    #[test]
    fn a_stray_release_says_nothing() {
        let mut button = HoldToTalk::default();
        assert_eq!(button.on_release(Instant::now(), HOLD, true), None);
    }

    #[test]
    fn commands_spell_the_voxtype_cli() {
        assert_eq!(DictateCommand::Start.line(), "voxtype record start");
        assert_eq!(DictateCommand::Stop.line(), "voxtype record stop");
        assert_eq!(
            DictateCommand::StopSubmit.line(),
            "voxtype record toggle --auto-submit"
        );
    }

    #[test]
    fn hold_and_submit_come_from_the_config_with_tuned_defaults() {
        let config: amon_protocol::Config =
            toml::from_str("[devices.micro2.dictation]\nhold_ms = 400\nauto_submit = false")
                .expect("parses");
        assert_eq!(hold_of(&config.devices.micro2), Duration::from_millis(400));
        assert!(!auto_submit_of(&config.devices.micro2));

        let absent = amon_protocol::Micro2Config::default();
        assert_eq!(hold_of(&absent), Duration::from_millis(250));
        assert!(auto_submit_of(&absent), "a hold submits unless told not to");
    }

    #[test]
    fn ducks_only_while_the_microphone_is_live() {
        assert!(microphone_live("recording"));
        assert!(microphone_live("streaming"));
        assert!(!microphone_live("idle"));
        assert!(!microphone_live("transcribing"));
        assert!(!microphone_live(""));
    }

    #[test]
    fn state_file_content_is_trimmed() {
        // The file is written with a trailing newline.
        assert!(microphone_live("recording\n"));
    }

    #[test]
    fn state_lives_in_the_runtime_dir() {
        std::env::set_var("XDG_RUNTIME_DIR", "/run/user/1000");
        assert_eq!(
            state_path(),
            Some(PathBuf::from("/run/user/1000/voxtype/state"))
        );
    }

    #[test]
    fn no_runtime_dir_means_no_state_file() {
        std::env::remove_var("XDG_RUNTIME_DIR");
        assert_eq!(state_path(), None);
    }

    #[test]
    fn the_config_file_spells_the_setting() {
        let config: amon_protocol::Config =
            toml::from_str("[sound]\nduck_while_dictating = true").expect("parses");
        assert!(config.sound.duck_while_dictating);
        let config: amon_protocol::Config =
            toml::from_str("[sound]\nduck_while_dictating = false").expect("parses");
        assert!(!config.sound.duck_while_dictating);
    }
}
