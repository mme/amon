//! Characterises what a coding agent puts on screen, through amon's own eyes.
//!
//! Spawns the agent on a PTY, types one prompt that forces a tool call, and
//! feeds every byte into a `ShadowTerminal` — the same emulator the wrapper
//! runs. It prints the *rendered* screen whenever it changes, and saves the
//! raw stream beside it.
//!
//! Rendering rather than grepping the byte stream is the whole point. An agent
//! writes a marker and its text as separate cursor operations, so the stream
//! makes a stable row look like it is flickering between two states; the screen
//! shows the row the human actually sees. Every conclusion about what a harness
//! surfaces has to be drawn from this side of the emulator.
//!
//! The saved stream replays to a byte-identical screen forever, which is what
//! lets an extraction rule be tested in CI without running an agent.
//!
//!   cargo run -p amon-term --example harness-probe -- claude [prompt]

use std::io::{Read, Write};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use amon_term::ShadowTerminal;
use portable_pty::{native_pty_system, CommandBuilder, PtySize};

const COLS: u16 = 100;
const ROWS: u16 = 30;
/// Long enough for a tool call to start, run and settle.
const RUN: Duration = Duration::from_secs(45);
const TYPE_AT: Duration = Duration::from_secs(8);
const ENTER_AT: Duration = Duration::from_secs(11);

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let agent = args.next().unwrap_or_else(|| "claude".into());
    let prompt = args.next().unwrap_or_else(|| {
        "Read the file NOTICE and tell me the licence name. Nothing else.".into()
    });

    let pair = native_pty_system().openpty(PtySize {
        rows: ROWS,
        cols: COLS,
        pixel_width: 0,
        pixel_height: 0,
    })?;

    let mut cmd = CommandBuilder::new(&agent);
    // The probe must not be wrapped by amon, and must not inherit this
    // session's identity: a nested agent that sees CLAUDE_CODE_* markers
    // disables its own transcript and behaves unlike a real one.
    for (key, _) in std::env::vars() {
        if key.starts_with("AMON_") || key.starts_with("CLAUDE_CODE_") {
            cmd.env_remove(&key);
        }
    }
    let path = std::env::var("PATH").unwrap_or_default();
    cmd.env(
        "PATH",
        path.split(':')
            .filter(|part| !part.contains("amon/shims"))
            .collect::<Vec<_>>()
            .join(":"),
    );
    // portable-pty defaults an unset cwd to $HOME, which would run the agent
    // outside the repo and change what its answer looks like.
    cmd.cwd(std::env::current_dir()?);
    cmd.env("COLUMNS", COLS.to_string());
    cmd.env("LINES", ROWS.to_string());
    let mut child = pair.slave.spawn_command(cmd)?;
    drop(pair.slave);

    let mut reader = pair.master.try_clone_reader()?;
    let (tx, rx) = mpsc::channel::<Vec<u8>>();
    std::thread::spawn(move || {
        let mut buf = [0u8; 65536];
        while let Ok(read) = reader.read(&mut buf) {
            if read == 0 || tx.send(buf[..read].to_vec()).is_err() {
                break;
            }
        }
    });
    let mut writer = pair.master.take_writer()?;

    let mut shadow = ShadowTerminal::new(COLS, ROWS)?;
    let mut raw: Vec<u8> = Vec::new();
    let mut last = String::new();
    let (mut typed, mut entered) = (false, false);
    let start = Instant::now();

    while start.elapsed() < RUN {
        if let Ok(chunk) = rx.recv_timeout(Duration::from_millis(100)) {
            raw.extend_from_slice(&chunk);
            shadow.feed(&chunk);
        }
        let elapsed = start.elapsed();
        if !typed && elapsed > TYPE_AT {
            writer.write_all(prompt.as_bytes())?;
            writer.flush()?;
            typed = true;
        } else if !entered && elapsed > ENTER_AT {
            writer.write_all(b"\r")?;
            writer.flush()?;
            entered = true;
        }

        let screen = shadow.detection_text();
        if screen != last {
            println!("\n=== t={:>6.2}s ===", elapsed.as_secs_f32());
            for line in screen.lines() {
                if !line.trim().is_empty() {
                    println!("|{line}");
                }
            }
            last = screen;
        }
    }

    let _ = child.kill();
    let _ = child.wait();
    let out = format!("/tmp/claude-1000/harness-{agent}.raw");
    std::fs::write(&out, &raw)?;
    eprintln!("\nsaved {} bytes to {out}", raw.len());
    Ok(())
}
