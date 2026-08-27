//! What a control does when touched: the action vocabulary and its engine.
//!
//! Actions are strings in the config (`key:super+left`, `exec:...`), parsed
//! once per config read. Execution goes through three doors: a virtual
//! uinput device for synthesized keys and scroll (covered by the same udev
//! rule that grants the hidraw access), `hyprctl` for window and workspace
//! moves, and plain process spawning for `exec`.

use std::io::Write;
use std::process::{Command, Stdio};

/// One control's meaning. Parsed from the config's action strings.
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    /// Inert. Also what an unknown string parses to, loudly logged.
    None,
    /// Toggle the Super+A agent panel.
    Panel,
    /// One wheel tick. Not in the config vocabulary — it is the encoder's
    /// fixed job, and a scroll without a direction means nothing on a key.
    Scroll,
    /// Go to a fixed workspace.
    Workspace(u32),
    /// Synthesize a key chord, e.g. `key:super+shift+f` or `key:Up`.
    Key(Vec<u16>),
    /// Run a command, through sh so the config line can be a pipeline.
    Exec(String),
}

impl Action {
    pub fn parse(text: &str) -> Action {
        let text = text.trim();
        match text {
            "none" | "" => Action::None,
            "panel" => Action::Panel,
            _ => {
                if let Some(rest) = text.strip_prefix("workspace:") {
                    match rest.trim().parse() {
                        Ok(n) if (1..=10).contains(&n) => Action::Workspace(n),
                        _ => Action::None,
                    }
                } else if let Some(rest) = text.strip_prefix("key:") {
                    match parse_chord(rest) {
                        Some(codes) => Action::Key(codes),
                        None => Action::None,
                    }
                } else if let Some(rest) = text.strip_prefix("exec:") {
                    Action::Exec(rest.trim().to_string())
                } else {
                    Action::None
                }
            }
        }
    }
}

/// `super+shift+f` → uinput key codes, modifiers first. `None` when any
/// part is unknown — half a chord is worse than none.
fn parse_chord(spec: &str) -> Option<Vec<u16>> {
    let mut codes = Vec::new();
    for part in spec.split('+') {
        codes.push(key_code(part.trim())?);
    }
    (!codes.is_empty()).then_some(codes)
}

/// Key names to Linux input key codes. Case-insensitive; covers the keys a
/// macro pad plausibly sends plus the full letter/digit/function rows.
fn key_code(name: &str) -> Option<u16> {
    let name = name.to_ascii_lowercase();
    let code = match name.as_str() {
        "esc" | "escape" => 1,
        "enter" | "return" => 28,
        "tab" => 15,
        "space" => 57,
        "backspace" => 14,
        "delete" | "del" => 111,
        "up" => 103,
        "down" => 108,
        "left" => 105,
        "right" => 106,
        "home" => 102,
        "end" => 107,
        "pageup" => 104,
        "pagedown" => 109,
        "super" | "meta" | "win" => 125,
        "ctrl" | "control" => 29,
        "alt" => 56,
        "shift" => 42,
        "minus" => 12,
        "equal" => 13,
        "comma" => 51,
        "period" | "dot" => 52,
        _ => {
            if let Some(number) = name.strip_prefix('f').and_then(|n| n.parse::<u16>().ok()) {
                return match number {
                    1..=10 => Some(58 + number),
                    11 => Some(87),
                    12 => Some(88),
                    13..=24 => Some(170 + number),
                    _ => None,
                };
            }
            if name.len() == 1 {
                let c = name.as_bytes()[0];
                const LETTERS: [u16; 26] = [
                    30, 48, 46, 32, 18, 33, 34, 35, 23, 36, 37, 38, 50, 49, 24, 25, 16, 19, 31, 20,
                    22, 47, 17, 45, 21, 44,
                ];
                if c.is_ascii_lowercase() {
                    return Some(LETTERS[(c - b'a') as usize]);
                }
                if c.is_ascii_digit() {
                    // KEY_1..KEY_9 are 2..10, KEY_0 is 11.
                    return Some(if c == b'0' { 11 } else { (c - b'0' + 1) as u16 });
                }
            }
            return None;
        }
    };
    Some(code)
}

// ---------------------------------------------------------------------------
// The virtual input device. The legacy uinput interface, spoken with libc
// alone: write a uinput_user_dev, ioctl the event bits on, create, then
// write input_event structs. Torn down with the daemon.
// ---------------------------------------------------------------------------

const EV_SYN: u16 = 0;
const EV_KEY: u16 = 1;
const EV_REL: u16 = 2;
const REL_WHEEL: u16 = 8;

const UI_SET_EVBIT: libc::c_ulong = 0x4004_5564;
const UI_SET_KEYBIT: libc::c_ulong = 0x4004_5565;
const UI_SET_RELBIT: libc::c_ulong = 0x4004_5566;
const UI_DEV_CREATE: libc::c_ulong = 0x5501;
const UI_DEV_DESTROY: libc::c_ulong = 0x5502;

pub struct VirtualInput {
    file: std::fs::File,
}

impl VirtualInput {
    /// Creates the device, or explains why not — the caller degrades to
    /// key/scroll actions doing nothing, which doctor reports.
    pub fn open() -> std::io::Result<VirtualInput> {
        use std::os::fd::AsRawFd;

        let file = std::fs::OpenOptions::new()
            .write(true)
            .open("/dev/uinput")?;
        let fd = file.as_raw_fd();
        let set = |request: libc::c_ulong, value: libc::c_int| -> std::io::Result<()> {
            if unsafe { libc::ioctl(fd, request as _, value) } < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        };
        set(UI_SET_EVBIT, EV_KEY as i32)?;
        set(UI_SET_EVBIT, EV_REL as i32)?;
        set(UI_SET_RELBIT, REL_WHEEL as i32)?;
        // Every key the chord table can name.
        for code in 1..=194 {
            set(UI_SET_KEYBIT, code)?;
        }

        // The legacy uinput_user_dev: 80-byte name, input_id, ff_effects_max,
        // then four 64-entry i32 abs arrays. 1116 bytes of mostly zeros.
        let mut setup = [0u8; 1116];
        let name = b"amon virtual input";
        setup[..name.len()].copy_from_slice(name);
        // input_id: bustype 0x06 (virtual), vendor/product zero.
        setup[80] = 0x06;
        let mut file_ref = &file;
        file_ref.write_all(&setup)?;
        if unsafe { libc::ioctl(fd, UI_DEV_CREATE as _) } < 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(VirtualInput { file })
    }

    fn emit(&mut self, kind: u16, code: u16, value: i32) -> std::io::Result<()> {
        // struct input_event on 64-bit: two u64 (timeval, kernel fills on 0),
        // then type, code, value.
        let mut event = [0u8; 24];
        event[16..18].copy_from_slice(&kind.to_ne_bytes());
        event[18..20].copy_from_slice(&code.to_ne_bytes());
        event[20..24].copy_from_slice(&value.to_ne_bytes());
        self.file.write_all(&event)
    }

    fn syn(&mut self) -> std::io::Result<()> {
        self.emit(EV_SYN, 0, 0)
    }

    /// Presses the chord down in order and releases it in reverse — the way
    /// fingers do, which is what applications are written against. On any
    /// failure, whatever went down still comes up: a chord that dies
    /// half-way must not leave super or shift logically held on a virtual
    /// keyboard that outlives the error.
    pub fn tap(&mut self, chord: &[u16]) -> std::io::Result<()> {
        let mut pressed: Vec<u16> = Vec::with_capacity(chord.len());
        let mut result = Ok(());
        for &code in chord {
            match self.emit(EV_KEY, code, 1) {
                Ok(()) => pressed.push(code),
                Err(error) => {
                    result = Err(error);
                    break;
                }
            }
        }
        if result.is_ok() {
            result = self.syn();
        }
        for &code in pressed.iter().rev() {
            let release = self.emit(EV_KEY, code, 0);
            if result.is_ok() {
                result = release;
            }
        }
        let syn = self.syn();
        if result.is_ok() {
            result = syn;
        }
        result
    }

    /// One wheel tick; positive scrolls up.
    pub fn scroll(&mut self, direction: i32) -> std::io::Result<()> {
        self.emit(EV_REL, REL_WHEEL, direction)?;
        self.syn()
    }
}

impl Drop for VirtualInput {
    fn drop(&mut self) {
        use std::os::fd::AsRawFd;
        unsafe { libc::ioctl(self.file.as_raw_fd(), UI_DEV_DESTROY as _) };
    }
}

// ---------------------------------------------------------------------------
// The other two doors.
// ---------------------------------------------------------------------------

/// Fire-and-forget children still have exit statuses, and a daemon that
/// never waits on them accrues a zombie per keypress. One detached thread
/// per child is cheap at human input rates and keeps the process table
/// honest.
pub fn reap(child: std::io::Result<std::process::Child>) {
    if let Ok(mut child) = child {
        std::thread::spawn(move || {
            let _ = child.wait();
        });
    }
}

/// Dispatches through Hyprland's Lua CLI, the same dialect `amon focus`
/// speaks. Fire and forget: a compositor that is not there is not an error
/// worth a log per keypress.
pub fn hyprctl(dispatch: &str) {
    reap(
        Command::new("hyprctl")
            .args(["dispatch", dispatch])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn(),
    );
}

/// Runs amon's own CLI. Reaching an agent is amon's sequence to own — the
/// window, and the pane inside it when the agent lives in a herdr session —
/// so the device asks for it by name rather than reimplementing it against
/// the compositor. `current_exe` first: a daemon started from a build
/// directory must not shell out to whatever `amon` happens to be on PATH.
pub fn amon(arguments: &[&str]) {
    let binary = std::env::current_exe().unwrap_or_else(|_| "amon".into());
    reap(
        Command::new(binary)
            .args(arguments)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn(),
    );
}

/// One window rectangle, for the joystick's edge guard.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl Rect {
    fn center(&self) -> (f64, f64) {
        (self.x + self.w / 2.0, self.y + self.h / 2.0)
    }
}

/// Whether any of `others` lies in `direction` from `active`, by center
/// comparison on the pushed axis. This is the guard that keeps a push at
/// the edge from falling back to some unrelated window: no neighbor, no
/// dispatch. Deliberately loose on the perpendicular axis — if the
/// compositor would find a diagonal-ish window, so do we.
pub fn has_neighbor(active: Rect, others: &[Rect], direction: (f64, f64)) -> bool {
    let (ax, ay) = active.center();
    others.iter().any(|other| {
        let (ox, oy) = other.center();
        let (dx, dy) = (ox - ax, oy - ay);
        dx * direction.0 + dy * direction.1 > 1.0
    })
}

/// Reads `hyprctl -j <command>` as JSON, `None` when the compositor is not
/// there or says something unparseable.
pub fn hyprctl_json(command: &str) -> Option<serde_json::Value> {
    let output = Command::new("hyprctl")
        .args(["-j", command])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| serde_json::from_slice(&output.stdout).ok())
        .flatten()
}

fn rect_of(window: &serde_json::Value) -> Option<Rect> {
    let at = window.get("at")?.as_array()?;
    let size = window.get("size")?.as_array()?;
    Some(Rect {
        x: at.first()?.as_f64()?,
        y: at.get(1)?.as_f64()?,
        w: size.first()?.as_f64()?,
        h: size.get(1)?.as_f64()?,
    })
}

/// Moves window focus in a direction — but only when there is somewhere to
/// go. A push at the edge does nothing, instead of the compositor's
/// fallback jumping focus somewhere unrelated. When the compositor cannot
/// be asked (queries fail), the dispatch goes through unguarded: a stick
/// that stops working is worse than one that wraps.
pub fn focus_direction(letter: &str, vector: (f64, f64)) {
    let guard = || -> Option<bool> {
        let active = hyprctl_json("activewindow")?;
        let active_rect = rect_of(&active)?;
        let workspace = active.get("workspace")?.get("id")?.as_i64()?;
        let address = active.get("address")?.as_str()?.to_string();
        let clients = hyprctl_json("clients")?;
        let rects: Vec<Rect> = clients
            .as_array()?
            .iter()
            .filter(|c| {
                c.get("workspace")
                    .and_then(|w| w.get("id"))
                    .and_then(|i| i.as_i64())
                    == Some(workspace)
                    && c.get("mapped").and_then(|m| m.as_bool()) == Some(true)
                    && c.get("address").and_then(|a| a.as_str()) != Some(address.as_str())
            })
            .filter_map(rect_of)
            .collect();
        Some(has_neighbor(active_rect, &rects, vector))
    };
    if guard() == Some(false) {
        return;
    }
    hyprctl(&format!("hl.dsp.focus({{ direction = \"{letter}\" }})"));
}

pub fn exec(command: &str) {
    reap(
        Command::new("sh")
            .args(["-c", command])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn(),
    );
}

/// Toggles the agent panel the way Super+A does.
pub fn toggle_panel() {
    reap(
        Command::new("omarchy-shell")
            .args(["-q", "shell", "toggle", "sh.amon.panel", ""])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn(),
    );
}

/// Offers an encoder event to the agent panel through the shell's `call`
/// IPC — the same seam the Super+W bind uses. True only when the panel
/// answered "handled" (the modal is up and took the event); a closed panel,
/// a shell that is not running, or an older plugin all read as false, and
/// the caller falls back to the knob's configured job.
pub fn panel_call(function: &str, argument: &str) -> bool {
    let output = Command::new("omarchy-shell")
        .args(["shell", "call", "sh.amon.panel", function, argument])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output();
    matches!(output, Ok(out) if out.status.success()
        && String::from_utf8_lossy(&out.stdout).trim() == "handled")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_vocabulary_parses() {
        assert_eq!(Action::parse("none"), Action::None);
        assert_eq!(Action::parse("panel"), Action::Panel);
        assert_eq!(Action::parse("workspace:3"), Action::Workspace(3));
        // Retired words: the fixed controls' jobs are no longer spellable.
        assert_eq!(Action::parse("agent"), Action::None);
        assert_eq!(Action::parse("scroll"), Action::None);
        assert_eq!(Action::parse("focus-direction"), Action::None);
        assert_eq!(
            Action::parse("exec:voxtype record toggle"),
            Action::Exec("voxtype record toggle".into())
        );
    }

    #[test]
    fn chords_resolve_modifiers_first_in_config_order() {
        assert_eq!(Action::parse("key:Up"), Action::Key(vec![103]));
        assert_eq!(
            Action::parse("key:super+shift+f"),
            Action::Key(vec![125, 42, 33])
        );
        assert_eq!(Action::parse("key:F13"), Action::Key(vec![183]));
        assert_eq!(Action::parse("key:Escape"), Action::Key(vec![1]));
    }

    #[test]
    fn nonsense_is_inert_never_partial() {
        // Half a chord pressed is worse than nothing pressed: a config typo
        // must not leave shift held down or fire the recognizable half.
        assert_eq!(Action::parse("key:super+florp"), Action::None);
        assert_eq!(Action::parse("workspace:0"), Action::None);
        assert_eq!(Action::parse("workspace:11"), Action::None);
        assert_eq!(Action::parse("frobnicate"), Action::None);
    }

    #[test]
    fn function_keys_map_across_their_three_ranges() {
        // F1-F10, F11/F12, and F13-F24 are three discontiguous ranges in the
        // kernel's code space; the seams are where a table goes wrong.
        assert_eq!(key_code("f1"), Some(59));
        assert_eq!(key_code("f10"), Some(68));
        assert_eq!(key_code("f11"), Some(87));
        assert_eq!(key_code("f12"), Some(88));
        assert_eq!(key_code("f13"), Some(183));
        assert_eq!(key_code("f24"), Some(194));
        assert_eq!(key_code("f25"), None);
    }

    #[test]
    fn the_edge_guard_knows_when_nothing_is_there() {
        let active = Rect {
            x: 0.0,
            y: 0.0,
            w: 100.0,
            h: 100.0,
        };
        let right_of = Rect {
            x: 200.0,
            y: 10.0,
            w: 100.0,
            h: 100.0,
        };
        let below = Rect {
            x: 0.0,
            y: 200.0,
            w: 100.0,
            h: 100.0,
        };
        let others = [right_of, below];

        assert!(has_neighbor(active, &others, (1.0, 0.0)), "right exists");
        assert!(has_neighbor(active, &others, (0.0, 1.0)), "down exists");
        assert!(
            !has_neighbor(active, &others, (-1.0, 0.0)),
            "nothing left: the push must be swallowed, not wrapped"
        );
        assert!(!has_neighbor(active, &others, (0.0, -1.0)), "nothing up");
        assert!(
            !has_neighbor(active, &[], (1.0, 0.0)),
            "a lone window swallows every push"
        );
    }

    #[test]
    fn digits_wrap_the_zero() {
        assert_eq!(key_code("1"), Some(2));
        assert_eq!(key_code("9"), Some(10));
        assert_eq!(key_code("0"), Some(11));
    }
}
