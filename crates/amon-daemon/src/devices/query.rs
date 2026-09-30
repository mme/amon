//! One question to the board, one answer: the request/reply half of the
//! Creator Micro 2's vendor channel, for callers that are not the device
//! loop — `amon doctor` asking what firmware and layer it is on, `amon
//! setup` reading and writing the keymap.
//!
//! hidraw hands every open descriptor its own copy of each report, so this
//! can run beside the daemon's loop on the same node: the loop sees our
//! replies as messages with an id it did not send, and ignores them, exactly
//! as it ignores the replies to its own lighting writes.

use std::io::{self, Read, Write};
use std::os::fd::AsRawFd;
use std::path::Path;
use std::time::{Duration, Instant};

use super::micro2::frame;

const REPORT_ID: u8 = 6;
const CHANNEL_RPC: u8 = 2;
/// Ids the device loop never uses (it cycles 1..=998), so a reply can be
/// told apart from its traffic.
const FIRST_ID: u32 = 9000;

/// An open vendor node and the half-read line behind it.
pub struct Board {
    file: std::fs::File,
    buffer: Vec<u8>,
    next_id: u32,
}

/// What the firmware said, without a transport error in between.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Answer {
    Result(serde_json::Value),
    /// The firmware's own error object: `{"code":404,"message":"..."}`.
    Error(serde_json::Value),
}

impl Board {
    pub fn open(node: &Path) -> io::Result<Self> {
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(node)?;
        Ok(Self {
            file,
            buffer: Vec::new(),
            next_id: FIRST_ID,
        })
    }

    /// Sends one request and waits up to `timeout` for the reply carrying
    /// its id. Notifications and other replies that arrive meanwhile are
    /// dropped: this is a question, not a listener.
    pub fn ask(
        &mut self,
        method: &str,
        params: Option<serde_json::Value>,
        timeout: Duration,
    ) -> io::Result<Answer> {
        let id = self.next_id;
        self.next_id += 1;
        let mut request = serde_json::json!({ "method": method, "id": id });
        if let Some(params) = params {
            request["params"] = params;
        }
        let mut payload = serde_json::to_vec(&request)?;
        payload.push(b'\n');
        for report in frame(&payload) {
            self.file.write_all(&report)?;
        }

        let deadline = Instant::now() + timeout;
        loop {
            if let Some(message) = self.next_message(deadline)? {
                if message.get("id").and_then(serde_json::Value::as_u64) != Some(u64::from(id)) {
                    continue;
                }
                if let Some(error) = message.get("error") {
                    return Ok(Answer::Error(error.clone()));
                }
                return Ok(Answer::Result(
                    message
                        .get("result")
                        .cloned()
                        .unwrap_or(serde_json::Value::Null),
                ));
            }
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                format!("no reply to {method} within {}ms", timeout.as_millis()),
            ));
        }
    }

    /// The next complete JSON line on the RPC channel, or `None` at the
    /// deadline.
    fn next_message(&mut self, deadline: Instant) -> io::Result<Option<serde_json::Value>> {
        loop {
            if let Some(newline) = self.buffer.iter().position(|b| *b == b'\n') {
                let line: Vec<u8> = self.buffer.drain(..=newline).collect();
                let text = String::from_utf8_lossy(&line[..line.len() - 1]);
                match serde_json::from_str::<serde_json::Value>(text.trim()) {
                    Ok(message) => return Ok(Some(message)),
                    Err(_) => continue,
                }
            }
            let now = Instant::now();
            if now >= deadline {
                return Ok(None);
            }
            let wait = (deadline - now).as_millis().min(i32::MAX as u128) as i32;
            let mut poll = libc::pollfd {
                fd: self.file.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            };
            let ready = unsafe { libc::poll(&mut poll, 1, wait) };
            if ready < 0 {
                return Err(io::Error::last_os_error());
            }
            if ready == 0 {
                return Ok(None);
            }
            let mut report = [0u8; 64];
            let read = self.file.read(&mut report)?;
            if read < 3 || report[0] != REPORT_ID || report[1] != CHANNEL_RPC {
                continue;
            }
            let length = (report[2] as usize).min(read.saturating_sub(3));
            self.buffer.extend_from_slice(&report[3..3 + length]);
            if self.buffer.len() > 64 * 1024 {
                self.buffer.clear();
            }
        }
    }
}

/// How long one answer may take. Bluetooth is the slow face; the daemon's
/// own probe allows two seconds and has not been wrong.
pub const ANSWER_TIMEOUT: Duration = Duration::from_millis(1500);

/// The firmware version and the active layer, as the board reports them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Status {
    pub firmware: String,
    /// 1-based, as `device.status` counts — the number printed beside the
    /// layer in Work Louder's own app, and the one the touch sensor's LEDs
    /// show.
    pub layer: usize,
}

pub fn status(board: &mut Board) -> io::Result<Status> {
    let answer = board.ask("device.status", None, ANSWER_TIMEOUT)?;
    let Answer::Result(result) = answer else {
        return Err(io::Error::other("device.status refused"));
    };
    let firmware = result
        .get("version")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("unknown")
        .trim_start_matches('v')
        .to_string();
    let layer = result
        .get("layer_index")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0) as usize;
    Ok(Status { firmware, layer })
}

const KEYMAP_FILE: &str = "keymap.json";

/// The whole on-flash configuration, parsed.
///
/// Asked twice before giving up: the file is some twenty-five reports long,
/// and a board busy with the daemon's first lighting burst has been seen to
/// miss the deadline once and answer at once the second time.
pub fn read_keymap(board: &mut Board) -> io::Result<serde_json::Value> {
    let params = serde_json::json!({ "file": KEYMAP_FILE });
    let answer = match board.ask("fs.read", Some(params.clone()), Duration::from_secs(4)) {
        Ok(answer) => answer,
        Err(error) if error.kind() == io::ErrorKind::TimedOut => {
            board.ask("fs.read", Some(params), Duration::from_secs(4))?
        }
        Err(error) => return Err(error),
    };
    let Answer::Result(result) = answer else {
        return Err(io::Error::other("fs.read refused"));
    };
    // The file comes back as a JSON string inside `data`.
    let data = result
        .get("data")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| io::Error::other("fs.read answered without data"))?;
    serde_json::from_str(data).map_err(|error| io::Error::other(format!("keymap.json: {error}")))
}

/// Writes the whole configuration back. The firmware applies it live — no
/// reboot — which is also why the caller keeps a copy of what was there.
pub fn write_keymap(board: &mut Board, keymap: &serde_json::Value) -> io::Result<()> {
    let data = serde_json::to_string(keymap)?;
    let answer = board.ask(
        "fs.write",
        Some(serde_json::json!({ "file": KEYMAP_FILE, "data": data })),
        Duration::from_secs(6),
    )?;
    match answer {
        Answer::Result(_) => Ok(()),
        Answer::Error(error) => Err(io::Error::other(format!("fs.write refused: {error}"))),
    }
}
