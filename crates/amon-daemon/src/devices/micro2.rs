//! The Work Louder Creator Micro 2: six agent keys lit by agent state, a
//! ring showing the fleet's most urgent state, and every control mapped to
//! an action (ADR-0016).
//!
//! The protocol is the one the Codex desktop app speaks, learned from its
//! published artifacts for interoperability and verified against real
//! hardware: JSON-RPC in 64-byte vendor-HID reports (report id 6, channel 2),
//! newline-delimited, one request in flight at a time. `v.oai.thstatus`
//! lights the six keys individually, `v.oai.rgbcfg` drives the ambient
//! ring, and the device volunteers `v.oai.hid` / `v.oai.rad` notifications
//! for every key, encoder detent, and joystick deflection — no handshake,
//! no mode to enter. The first lighting write doubles as the capability
//! probe: firmware that predates the protocol times out and is left alone.

use std::io::{Read, Write};
use std::sync::mpsc::{Receiver, Sender};
use std::time::{Duration, Instant};

use amon_protocol::{AgentEntry, AgentState, Micro2Config};

use super::actions::{self, Action, VirtualInput};

const REPORT_ID: u8 = 6;
const CHANNEL_RPC: u8 = 2;

/// The Codex palette, the tuned default (`[devices.micro2.colors]` overrides).
const COLOR_BLOCKED: u32 = 0xFF6D00;
const COLOR_WORKING: u32 = 0x304FFE;
const COLOR_DONE: u32 = 0x00FF4C;
const COLOR_IDLE: u32 = 0xFFFFFF;

#[cfg(test)]
const EFFECT_OFF: u8 = 0;
const EFFECT_SOLID: u8 = 1;
const EFFECT_SNAKE: u8 = 2;
const EFFECT_BREATH: u8 = 4;

/// What one key shows. Everything the wire wants, nothing it does not.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct KeyLight {
    pub color: u32,
    pub effect: u8,
    /// Per-mille, wire-ready: brightness crosses the wire as 0.0–1.0 but is
    /// held integral here so lighting states stay comparable for dedup.
    pub brightness_pm: u16,
    pub speed_pm: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RingLight {
    pub color: u32,
    pub effect: u8,
    pub brightness_pm: u16,
    pub speed_pm: u16,
}

/// The whole visible state, computed pure from agents + config, compared
/// for dedup, then serialized. Two identical computations never reach the
/// device twice.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Lighting {
    pub keys: [KeyLight; 6],
    pub ring: RingLight,
}

fn parse_color(text: &str, fallback: u32) -> u32 {
    let hex = text.trim().trim_start_matches('#');
    if hex.len() == 6 {
        if let Ok(value) = u32::from_str_radix(hex, 16) {
            return value;
        }
    }
    fallback
}

fn state_color(state: AgentState, seen: Option<bool>, config: &Micro2Config) -> (u32, bool) {
    let colors = &config.colors;
    match state {
        AgentState::Blocked => (
            colors
                .blocked
                .as_deref()
                .map_or(COLOR_BLOCKED, |c| parse_color(c, COLOR_BLOCKED)),
            false,
        ),
        AgentState::Working => (
            colors
                .working
                .as_deref()
                .map_or(COLOR_WORKING, |c| parse_color(c, COLOR_WORKING)),
            true,
        ),
        AgentState::Idle if seen == Some(false) => (
            colors
                .done
                .as_deref()
                .map_or(COLOR_DONE, |c| parse_color(c, COLOR_DONE)),
            false,
        ),
        _ => (
            colors
                .idle
                .as_deref()
                .map_or(COLOR_IDLE, |c| parse_color(c, COLOR_IDLE)),
            false,
        ),
    }
}

fn brightness_pm(config: &Micro2Config) -> u16 {
    let value = config.brightness.unwrap_or(1.0).clamp(0.0, 1.0);
    (value * 1000.0) as u16
}

/// The most urgent agent on a workspace, by the ranking every other surface
/// uses ([`AgentEntry::attention`]).
fn most_urgent(agents: &[AgentEntry]) -> Option<&AgentEntry> {
    agents
        .iter()
        .min_by_key(|agent| (agent.attention(), agent.state_since))
}

/// Computes the six keys and the ring from the agent roster. Pure — this is
/// where the tests live, and the transport just ships whatever this says.
pub fn lighting(agents: &[AgentEntry], config: &Micro2Config) -> Lighting {
    let brightness = brightness_pm(config);
    let mut keys = [KeyLight::default(); 6];

    // Key N is workspace N's most urgent agent, always — the same logic as
    // the workspace switcher, and deliberately not configurable: the keys
    // mean the same thing on every desk, like the number row does.
    let key_agents: Vec<Option<&AgentEntry>> = (0..6)
        .map(|i| {
            let workspace = (i + 1).to_string();
            let on_workspace: Vec<AgentEntry> = agents
                .iter()
                .filter(|agent| agent.workspace.as_deref() == Some(workspace.as_str()))
                .cloned()
                .collect();
            most_urgent(&on_workspace).and_then(|urgent| agents.iter().find(|a| a.id == urgent.id))
        })
        .collect();

    for (slot, agent) in key_agents.iter().enumerate() {
        let Some(agent) = agent else {
            continue; // dark key
        };
        let (color, breathes) = state_color(agent.state, agent.seen, config);
        keys[slot] = KeyLight {
            color,
            effect: if breathes {
                EFFECT_BREATH
            } else {
                EFFECT_SOLID
            },
            brightness_pm: brightness,
            speed_pm: if breathes { 400 } else { 0 },
        };
    }

    let ring = if config.ring.unwrap_or(true) {
        fleet_ring(agents, config, brightness)
    } else {
        RingLight::default()
    };

    Lighting { keys, ring }
}

/// The ring answers "does anything, anywhere, need me": solid blocked color
/// when anything waits for a human, breathing done color when something
/// finished unseen, snaking working color while anything works, else off.
fn fleet_ring(agents: &[AgentEntry], config: &Micro2Config, brightness: u16) -> RingLight {
    let any = |predicate: &dyn Fn(&&AgentEntry) -> bool| agents.iter().any(|a| predicate(&a));
    if any(&|a| a.state == AgentState::Blocked) {
        let (color, _) = state_color(AgentState::Blocked, None, config);
        return RingLight {
            color,
            effect: EFFECT_SOLID,
            brightness_pm: brightness,
            speed_pm: 0,
        };
    }
    if any(&|a| a.state == AgentState::Idle && a.seen == Some(false)) {
        let (color, _) = state_color(AgentState::Idle, Some(false), config);
        return RingLight {
            color,
            effect: EFFECT_BREATH,
            brightness_pm: brightness,
            speed_pm: 400,
        };
    }
    if any(&|a| a.state == AgentState::Working) {
        let (color, _) = state_color(AgentState::Working, None, config);
        return RingLight {
            color,
            effect: EFFECT_SNAKE,
            brightness_pm: brightness,
            speed_pm: 400,
        };
    }
    RingLight::default()
}

/// Which agent a key tap goes to, and the workspace the key stands for
/// when no agent is on it (a plain workspace switch).
pub fn tap_target(slot: usize, agents: &[AgentEntry]) -> (Option<AgentEntry>, Option<u32>) {
    let workspace = (slot + 1) as u32;
    let name = workspace.to_string();
    let on_workspace: Vec<AgentEntry> = agents
        .iter()
        .filter(|agent| agent.workspace.as_deref() == Some(name.as_str()))
        .cloned()
        .collect();
    (most_urgent(&on_workspace).cloned(), Some(workspace))
}

// ---------------------------------------------------------------------------
// Wire: framing and JSON building. Pure, tested.
// ---------------------------------------------------------------------------

/// Splits a payload into 64-byte reports: `[6, channel, len, bytes…]`.
pub fn frame(payload: &[u8]) -> Vec<[u8; 64]> {
    let mut reports = Vec::new();
    let mut offset = 0;
    while offset < payload.len() {
        let take = (payload.len() - offset).min(61);
        let mut report = [0u8; 64];
        report[0] = REPORT_ID;
        report[1] = CHANNEL_RPC;
        report[2] = take as u8;
        report[3..3 + take].copy_from_slice(&payload[offset..offset + take]);
        reports.push(report);
        offset += take;
    }
    reports
}

/// The thstatus request for a full six-key state.
pub fn thstatus_json(keys: &[KeyLight; 6], id: u32) -> String {
    let entries: Vec<String> = keys
        .iter()
        .enumerate()
        .map(|(slot, key)| {
            format!(
                r#"{{"id":{},"c":{},"b":{},"e":{},"s":{},"sk":0,"sa":0}}"#,
                slot,
                key.color,
                key.brightness_pm as f32 / 1000.0,
                key.effect,
                key.speed_pm as f32 / 1000.0,
            )
        })
        .collect();
    format!(
        r#"{{"method":"v.oai.thstatus","params":[{}],"id":{}}}"#,
        entries.join(","),
        id
    )
}

/// The rgbcfg request for the ring (keys zone left off — the six keys are
/// thstatus's business).
pub fn rgbcfg_json(ring: &RingLight, id: u32) -> String {
    format!(
        r#"{{"method":"v.oai.rgbcfg","params":{{"ambient":{{"e":{},"b":{},"s":{},"m":0,"c":{}}}}},"id":{}}}"#,
        ring.effect,
        ring.brightness_pm as f32 / 1000.0,
        ring.speed_pm as f32 / 1000.0,
        ring.color,
        id
    )
}

pub fn all_off_thstatus(id: u32) -> String {
    thstatus_json(&[KeyLight::default(); 6], id)
}

pub fn all_off_rgbcfg(id: u32) -> String {
    rgbcfg_json(&RingLight::default(), id)
}

// ---------------------------------------------------------------------------
// Input events, decoded from the device's notifications.
// ---------------------------------------------------------------------------

/// A control was touched. Joystick deflections arrive already digested into
/// a direction (the raw stream is angle+distance at report rate).
#[derive(Debug, Clone, PartialEq)]
pub enum DeviceInput {
    AgentKey(usize),
    Control(String),
    EncoderCw,
    EncoderCc,
    /// The raw stream; the device loop latches it into single
    /// [`DeviceInput::Joystick`] pushes.
    JoystickRaw {
        angle: f64,
        distance: f64,
    },
    Joystick(JoyDirection),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JoyDirection {
    Left,
    Right,
    Up,
    Down,
}

/// Decodes one notification (`method`/`m` with no id) into an input, if it
/// is one amon acts on. Presses act on `act:1`; releases and unknown keys
/// are `None`.
pub fn decode_notification(message: &serde_json::Value) -> Option<DeviceInput> {
    let method = message
        .get("method")
        .or_else(|| message.get("m"))?
        .as_str()?;
    let params = message.get("params").or_else(|| message.get("p"))?;
    match method {
        "v.oai.hid" => {
            let key = params.get("k")?.as_str()?;
            let act = params.get("act").and_then(|a| a.as_i64()).unwrap_or(1);
            match key {
                "ENC_CW" => Some(DeviceInput::EncoderCw),
                "ENC_CC" => Some(DeviceInput::EncoderCc),
                _ if act != 1 => None,
                _ => {
                    if let Some(slot) = key
                        .strip_prefix("AG0")
                        .and_then(|n| n.parse::<usize>().ok())
                        .filter(|n| *n < 6)
                    {
                        Some(DeviceInput::AgentKey(slot))
                    } else {
                        Some(DeviceInput::Control(key.to_string()))
                    }
                }
            }
        }
        "v.oai.rad" => {
            let angle = params.get("a")?.as_f64()?;
            let distance = params.get("d")?.as_f64()?;
            Some(DeviceInput::JoystickRaw { angle, distance })
        }
        _ => None,
    }
}

/// A deflection past the threshold reads as one of four directions; anything
/// less is noise. Angle is 0–1 around the circle, in the device's own
/// frame — which no amount of reasoning could orient. Settled by a labeled
/// four-push calibration on hardware (up=0.765, right=0.957, down=0.235,
/// left=0.488): right wraps at 0.0/1.0, down 0.25, left 0.5, up 0.75.
fn joystick_direction(angle: f64, distance: f64) -> Option<JoyDirection> {
    if distance < 0.8 {
        return None;
    }
    let angle = angle.rem_euclid(1.0);
    Some(if !(0.125..0.875).contains(&angle) {
        JoyDirection::Right
    } else if angle < 0.375 {
        JoyDirection::Down
    } else if angle < 0.625 {
        JoyDirection::Left
    } else {
        JoyDirection::Up
    })
}

/// The seven macro keys as the config spells them, from the wire's names:
/// numbered reading order, `macro_1`..`macro_4` across the upper row and
/// `macro_5`..`macro_7` across the lower. Only these are remappable; the
/// agent keys, encoder, and joystick have fixed roles.
fn control_config_name(key: &str) -> Option<&'static str> {
    Some(match key {
        "ACT06" => "macro_1",
        "ACT07" => "macro_2",
        "ACT08" => "macro_3",
        "ACT09" => "macro_4",
        "ACT10" => "macro_5",
        "ACT11" => "macro_6",
        "ACT12" => "macro_7",
        _ => return None,
    })
}

/// The action bound to a control under this config, defaults included.
pub fn action_for(control: &str, config: &Micro2Config) -> Action {
    let name = match control_config_name(control) {
        Some(name) => name,
        None => return Action::None,
    };
    if let Some(text) = config.keys.get(name) {
        return Action::parse(text);
    }
    let default = match name {
        "macro_1" => "panel",
        "macro_2" => "none",
        "macro_3" => "key:Up",
        "macro_4" => "key:Escape",
        "macro_5" => "exec:voxtype record toggle",
        "macro_6" => "key:Down",
        "macro_7" => "key:Enter",
        _ => "none",
    };
    Action::parse(default)
}

/// Runs one decoded input against the config. `agents` is the roster the
/// lighting was last computed from, so a tap goes where the light said.
pub fn act(
    input: &DeviceInput,
    agents: &[AgentEntry],
    config: &Micro2Config,
    synth: &mut Option<VirtualInput>,
) {
    match input {
        DeviceInput::AgentKey(slot) => {
            let (agent, workspace) = tap_target(*slot, agents);
            match (agent, workspace) {
                (Some(agent), _) => focus_agent(&agent),
                (None, Some(workspace)) => {
                    actions::hyprctl(&format!("hl.dsp.focus({{ workspace = \"{workspace}\" }})"));
                }
                _ => {}
            }
        }
        DeviceInput::Control(key) => {
            // The encoder's click is not remappable: while the panel is up
            // it activates the row the knob walked to; otherwise it does
            // nothing.
            if key == "ENC_CLK" {
                let _ = actions::panel_call("encoderSelect", "x");
                return;
            }
            let action = action_for(key, config);
            run_action(&action, None, synth);
        }
        // The knob, like its click, is the panel's while the panel is up —
        // a detent moves the selection instead of scrolling. The signs are
        // by feel on hardware: the detent that scrolls a page up is the one
        // that walks the list up.
        DeviceInput::EncoderCw => {
            if !actions::panel_call("encoderMove", "-1") {
                run_action(&Action::Scroll, Some(1), synth)
            }
        }
        DeviceInput::EncoderCc => {
            if !actions::panel_call("encoderMove", "1") {
                run_action(&Action::Scroll, Some(-1), synth)
            }
        }
        // Raw joystick readings never reach here: the device loop latches
        // them into single Joystick pushes first.
        DeviceInput::JoystickRaw { .. } => {}
        DeviceInput::Joystick(direction) => {
            let (letter, vector) = match direction {
                JoyDirection::Left => ("l", (-1.0, 0.0)),
                JoyDirection::Right => ("r", (1.0, 0.0)),
                JoyDirection::Up => ("u", (0.0, -1.0)),
                JoyDirection::Down => ("d", (0.0, 1.0)),
            };
            actions::focus_direction(letter, vector);
        }
    }
}

/// Retries the virtual device when it is missing. `amon setup` can grant
/// /dev/uinput access while a device is already attached, and a key that
/// stayed dead until the next replug would read as setup having failed. A
/// denied open fails immediately, so the retry costs a keypress nothing.
fn reopen_synth(synth: &mut Option<VirtualInput>) {
    if synth.is_none() {
        *synth = VirtualInput::open().ok();
    }
}

fn run_action(action: &Action, scroll_direction: Option<i32>, synth: &mut Option<VirtualInput>) {
    match action {
        Action::None => {}
        Action::Panel => actions::toggle_panel(),
        Action::Scroll => {
            reopen_synth(synth);
            if let (Some(direction), Some(device)) = (scroll_direction, synth.as_mut()) {
                let _ = device.scroll(direction);
            }
        }
        Action::Workspace(n) => {
            actions::hyprctl(&format!("hl.dsp.focus({{ workspace = \"{n}\" }})"))
        }
        Action::Key(chord) => {
            reopen_synth(synth);
            if let Some(device) = synth.as_mut() {
                let _ = device.tap(chord);
            }
        }
        Action::Exec(command) => actions::exec(command),
    }
}

/// Focus the agent the way the panel's Enter does — through `amon focus`,
/// never by dispatching the compositor from here. Reaching an agent is one
/// operation with more than one part: the window, and, for an agent living
/// in a herdr pane, the pane inside it. amon owns that sequence, so a key
/// that reached for hyprctl itself would land on the right window and the
/// wrong pane.
fn focus_agent(agent: &AgentEntry) {
    actions::amon(&["focus", "--agent", &agent.id]);
}

// ---------------------------------------------------------------------------
// The device loop: owns the hidraw fd, ships lighting, listens for input.
// ---------------------------------------------------------------------------

/// Messages the discovery loop sends an attached device.
pub enum ToDevice {
    /// The roster changed; recompute and ship if different.
    Agents(Vec<AgentEntry>),
    /// The config changed.
    Config(Box<Micro2Config>),
    /// The daemon is going down: all off, then stop.
    Shutdown,
}

/// How long a woken run gives the first probe. Codex marks unsupported
/// firmware within 50ms; a Bluetooth link deserves more patience than that,
/// and one slow answer is cheaper than a false negative.
const PROBE_TIMEOUT: Duration = Duration::from_secs(2);

/// How often the current lighting is re-sent even though nothing changed.
/// The dedup that keeps identical states off the wire has a blind spot: a
/// device that loses its volatile lighting state without dropping the
/// link — a Bluetooth hiccup under radio load is enough — stays dark until
/// an agent happens to change state, because amon believes the colors are
/// already showing. A periodic refresh is two small writes and heals that
/// silently.
const REFRESH_INTERVAL: Duration = Duration::from_secs(15);

/// Wire-level result the loop reports back for doctor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceOutcome {
    /// Detached normally (device gone, shutdown).
    Detached,
    /// The firmware never answered the probe: no lighting support.
    Unsupported,
}

/// Runs one attached device until it disappears or the daemon shuts down.
///
/// Reads and writes share the fd; reads run under a poll timeout so the
/// inbox is serviced regularly. Writes are deferred while device input is
/// arriving (the 100ms quiet Codex keeps) and identical states are never
/// re-sent.
pub fn run(
    mut file: std::fs::File,
    inbox: Receiver<ToDevice>,
    outcome_tx: Sender<DeviceOutcome>,
    mut config: Micro2Config,
) {
    use std::os::fd::AsRawFd;

    let mut synth = VirtualInput::open().ok();
    let mut agents: Vec<AgentEntry> = Vec::new();
    let mut shown: Option<Lighting> = None;
    let mut request_id: u32 = 1;
    let mut buffer: Vec<u8> = Vec::new();
    let mut last_input = Instant::now() - Duration::from_secs(1);
    let mut pending = true; // ship initial state
    let mut last_shipped = Instant::now();
    let mut probed = false;
    let mut heard_anything = false;
    let mut joystick_engaged = false;
    let started = Instant::now();

    let next_id = |request_id: &mut u32| {
        *request_id = (*request_id % 998) + 1;
        *request_id
    };

    loop {
        // Inbox first: newest roster/config wins before any wire work.
        loop {
            match inbox.try_recv() {
                Ok(ToDevice::Agents(list)) => {
                    agents = list;
                    pending = true;
                }
                Ok(ToDevice::Config(new)) => {
                    config = *new;
                    pending = true;
                }
                Ok(ToDevice::Shutdown) => {
                    let id = next_id(&mut request_id);
                    let _ = write_all(&mut file, &all_off_thstatus(id));
                    let id = next_id(&mut request_id);
                    let _ = write_all(&mut file, &all_off_rgbcfg(id));
                    let _ = outcome_tx.send(DeviceOutcome::Detached);
                    return;
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => break,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    let _ = outcome_tx.send(DeviceOutcome::Detached);
                    return;
                }
            }
        }

        // Ship lighting when it changed and the device has been quiet —
        // and re-ship it on a timer even when it has not, in case the
        // device forgot it (see REFRESH_INTERVAL).
        let refresh_due = last_shipped.elapsed() > REFRESH_INTERVAL;
        if (pending || refresh_due) && last_input.elapsed() > Duration::from_millis(100) {
            let next = if config.enabled {
                lighting(&agents, &config)
            } else {
                Lighting::default()
            };
            if refresh_due || shown.as_ref() != Some(&next) {
                let id = next_id(&mut request_id);
                if write_all(&mut file, &thstatus_json(&next.keys, id)).is_err() {
                    let _ = outcome_tx.send(DeviceOutcome::Detached);
                    return;
                }
                let id = next_id(&mut request_id);
                let _ = write_all(&mut file, &rgbcfg_json(&next.ring, id));
                shown = Some(next);
                last_shipped = Instant::now();
            }
            pending = false;
        }

        // The capability probe: if nothing error-free — no reply, no
        // notification — has *ever* arrived within the window after the
        // first write, the firmware does not speak the protocol. "Ever", not "right now":
        // replies are consumed by the parser the moment they land, so any
        // buffer-state check reads empty exactly when everything works.
        if !probed && started.elapsed() > PROBE_TIMEOUT {
            probed = true;
            if !heard_anything && shown.is_some() {
                let _ = outcome_tx.send(DeviceOutcome::Unsupported);
                return;
            }
        }

        // Read with a short poll so the inbox stays serviced.
        let mut poll = libc::pollfd {
            fd: file.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        let ready = unsafe { libc::poll(&mut poll, 1, 200) };
        if ready < 0 {
            let _ = outcome_tx.send(DeviceOutcome::Detached);
            return;
        }
        if ready == 0 {
            continue;
        }
        let mut report = [0u8; 64];
        let read = match file.read(&mut report) {
            Ok(0) | Err(_) => {
                let _ = outcome_tx.send(DeviceOutcome::Detached);
                return;
            }
            Ok(n) => n,
        };
        // Only the RPC channel carries what we parse; the firmware's debug
        // channel shares the report id and would interleave garbage into
        // the JSON stream.
        if read < 3 || report[0] != REPORT_ID || report[1] != CHANNEL_RPC {
            continue;
        }
        let length = (report[2] as usize).min(read.saturating_sub(3));
        buffer.extend_from_slice(&report[3..3 + length]);
        // A device that streams valid-looking reports without ever sending
        // a newline must not grow the buffer forever. Real messages are
        // well under a kilobyte; anything this size is not a message.
        if buffer.len() > 16 * 1024 {
            buffer.clear();
        }
        while let Some(newline) = buffer.iter().position(|b| *b == b'\n') {
            let line: Vec<u8> = buffer.drain(..=newline).collect();
            let Ok(text) = std::str::from_utf8(&line[..line.len() - 1]) else {
                continue;
            };
            let Ok(message) = serde_json::from_str::<serde_json::Value>(text.trim()) else {
                continue;
            };
            // Proof the firmware speaks the protocol — for the capability
            // probe — is a message *without* an error. Old firmware answers
            // the probe write with a method-not-found error, which proves
            // the opposite; counting it would leave us attached to a device
            // we cannot light.
            if message.get("error").is_none() {
                heard_anything = true;
            }
            if message.get("id").is_some() {
                continue; // an RPC reply; lighting is fire-and-forget
            }
            if let Some(input) = decode_notification(&message) {
                last_input = Instant::now();
                pending = true; // re-ship after the quiet window if needed
                                // The joystick streams continuously while deflected, angle
                                // drifting under a held thumb. One push is one move: fire on
                                // the first over-threshold reading, then stay quiet until
                                // the stick has clearly returned toward center.
                let input = match input {
                    DeviceInput::JoystickRaw { angle, distance } => {
                        if distance < 0.4 {
                            joystick_engaged = false;
                            continue;
                        }
                        if joystick_engaged {
                            continue;
                        }
                        match joystick_direction(angle, distance) {
                            Some(direction) => {
                                joystick_engaged = true;
                                DeviceInput::Joystick(direction)
                            }
                            None => continue,
                        }
                    }
                    other => other,
                };
                if config.enabled {
                    act(&input, &agents, &config, &mut synth);
                }
            }
        }
    }
}

fn write_all(file: &mut std::fs::File, json: &str) -> std::io::Result<()> {
    let mut payload = json.as_bytes().to_vec();
    payload.push(b'\n');
    for report in frame(&payload) {
        file.write_all(&report)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn agent(id: &str, workspace: &str, state: AgentState, seen: Option<bool>) -> AgentEntry {
        AgentEntry {
            id: id.into(),
            agent: "claude".into(),
            state,
            state_since: 100,
            cwd: "/tmp".into(),
            pid: 1,
            args: vec![],
            hostname: "host".into(),
            started_at: 1,
            title: None,
            agent_session_id: None,
            agent_session_path: None,
            window: Some("abc123".into()),
            workspace: Some(workspace.into()),
            branch: None,
            focused: None,
            seen,
            herdr: None,
        }
    }

    #[test]
    fn each_workspace_lights_its_key() {
        let agents = vec![
            agent("a", "1", AgentState::Working, Some(true)),
            agent("b", "3", AgentState::Blocked, Some(false)),
        ];
        let config = Micro2Config::default();

        let lit = lighting(&agents, &config);

        assert_eq!(lit.keys[0].color, COLOR_WORKING);
        assert_eq!(lit.keys[0].effect, EFFECT_BREATH, "working breathes");
        assert_eq!(lit.keys[1].effect, EFFECT_OFF, "empty workspace is dark");
        assert_eq!(lit.keys[2].color, COLOR_BLOCKED);
        assert_eq!(lit.keys[2].effect, EFFECT_SOLID);
    }

    #[test]
    fn the_most_urgent_agent_speaks_for_a_shared_workspace() {
        // Same rule as the bar: blocked outranks working outranks idle.
        let agents = vec![
            agent("calm", "2", AgentState::Idle, Some(true)),
            agent("loud", "2", AgentState::Blocked, Some(false)),
        ];

        let lit = lighting(&agents, &Micro2Config::default());

        assert_eq!(lit.keys[1].color, COLOR_BLOCKED);
    }

    #[test]
    fn finishing_unseen_is_its_own_color() {
        let agents = vec![agent("d", "1", AgentState::Idle, Some(false))];
        let lit = lighting(&agents, &Micro2Config::default());
        assert_eq!(lit.keys[0].color, COLOR_DONE);
    }

    #[test]
    fn the_ring_answers_for_the_whole_fleet() {
        let working = vec![agent("w", "1", AgentState::Working, Some(true))];
        assert_eq!(
            lighting(&working, &Micro2Config::default()).ring.effect,
            EFFECT_SNAKE,
            "anything working snakes the ring"
        );

        let blocked = vec![
            agent("w", "1", AgentState::Working, Some(true)),
            agent("b", "9", AgentState::Blocked, Some(false)),
        ];
        let ring = lighting(&blocked, &Micro2Config::default()).ring;
        assert_eq!(ring.color, COLOR_BLOCKED, "blocked outranks working");
        assert_eq!(ring.effect, EFFECT_SOLID);

        assert_eq!(
            lighting(&[], &Micro2Config::default()).ring.effect,
            EFFECT_OFF,
            "a quiet fleet is a dark ring"
        );
    }

    #[test]
    fn configured_colors_override_the_palette() {
        let mut config = Micro2Config::default();
        config.colors.blocked = Some("#123456".into());
        let agents = vec![agent("b", "1", AgentState::Blocked, None)];
        assert_eq!(lighting(&agents, &config).keys[0].color, 0x123456);
    }

    #[test]
    fn framing_splits_at_61_bytes_and_labels_every_report() {
        let payload: Vec<u8> = (0..130).map(|i| i as u8).collect();
        let reports = frame(&payload);
        assert_eq!(reports.len(), 3);
        assert!(reports.iter().all(|r| r[0] == 6 && r[1] == 2));
        assert_eq!(reports[0][2], 61);
        assert_eq!(reports[2][2], 8);
        let rebuilt: Vec<u8> = reports
            .iter()
            .flat_map(|r| r[3..3 + r[2] as usize].to_vec())
            .collect();
        assert_eq!(rebuilt, payload, "nothing lost at the seams");
    }

    #[test]
    fn notifications_decode_the_way_the_hardware_sends_them() {
        // Shapes captured live from a Creator Micro 2 on firmware 0.6.3.
        let tap: serde_json::Value =
            serde_json::from_str(r#"{"m":"v.oai.hid","p":{"k":"AG02","act":1}}"#).unwrap();
        assert_eq!(decode_notification(&tap), Some(DeviceInput::AgentKey(2)));

        let release: serde_json::Value =
            serde_json::from_str(r#"{"m":"v.oai.hid","p":{"k":"AG02","act":0}}"#).unwrap();
        assert_eq!(decode_notification(&release), None, "releases are silent");

        let detent: serde_json::Value =
            serde_json::from_str(r#"{"m":"v.oai.hid","p":{"k":"ENC_CW","act":2}}"#).unwrap();
        assert_eq!(decode_notification(&detent), Some(DeviceInput::EncoderCw));

        let macro_key: serde_json::Value =
            serde_json::from_str(r#"{"m":"v.oai.hid","p":{"k":"ACT10","act":1}}"#).unwrap();
        assert_eq!(
            decode_notification(&macro_key),
            Some(DeviceInput::Control("ACT10".into()))
        );

        let stick: serde_json::Value =
            serde_json::from_str(r#"{"m":"v.oai.rad","p":{"a":0.25,"d":1}}"#).unwrap();
        assert_eq!(
            decode_notification(&stick),
            Some(DeviceInput::JoystickRaw {
                angle: 0.25,
                distance: 1.0
            }),
            "the stream arrives raw; the device loop latches it into pushes"
        );
    }

    #[test]
    fn the_joystick_compass_matches_the_calibrated_angles() {
        // Settled by a labeled four-push calibration on hardware, after
        // guessing lost three times: measured up=0.765, right=0.957,
        // down=0.235, left=0.488. The angle lives in the device's own
        // frame; only labeled pushes could orient it.
        assert_eq!(joystick_direction(0.957, 1.0), Some(JoyDirection::Right));
        assert_eq!(joystick_direction(0.02, 1.0), Some(JoyDirection::Right));
        assert_eq!(joystick_direction(0.235, 1.0), Some(JoyDirection::Down));
        assert_eq!(joystick_direction(0.488, 1.0), Some(JoyDirection::Left));
        assert_eq!(joystick_direction(0.765, 1.0), Some(JoyDirection::Up));
        assert_eq!(
            joystick_direction(0.25, 0.3),
            None,
            "under-threshold is noise"
        );
    }

    #[test]
    fn defaults_place_the_requested_layout() {
        let config = Micro2Config::default();
        assert_eq!(action_for("ACT06", &config), Action::Panel);
        assert_eq!(action_for("ACT07", &config), Action::None);
        assert_eq!(action_for("ACT08", &config), Action::Key(vec![103]));
        assert_eq!(action_for("ACT09", &config), Action::Key(vec![1]));
        assert_eq!(
            action_for("ACT10", &config),
            Action::Exec("voxtype record toggle".into())
        );
        assert_eq!(action_for("ACT11", &config), Action::Key(vec![108]));
        assert_eq!(action_for("ACT12", &config), Action::Key(vec![28]));
    }

    #[test]
    fn config_overrides_rebind_only_the_macro_keys() {
        let mut config = Micro2Config::default();
        config.keys.insert("macro_2".into(), "exec:obsidian".into());
        // Entries for the fixed controls are dead letters, not errors.
        config.keys.insert("encoder".into(), "none".into());
        config.keys.insert("joystick".into(), "scroll".into());
        assert_eq!(
            action_for("ACT07", &config),
            Action::Exec("obsidian".into())
        );
        assert_eq!(action_for("ENC_CLK", &config), Action::None);
        assert_eq!(action_for("nonsense", &config), Action::None);
    }

    #[test]
    fn identical_lighting_states_compare_equal_for_dedup() {
        let agents = vec![agent("a", "1", AgentState::Working, Some(true))];
        let config = Micro2Config::default();
        assert_eq!(lighting(&agents, &config), lighting(&agents, &config));
    }
}
