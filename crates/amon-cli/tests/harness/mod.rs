//! Support for driving the real `amon` binary.
//!
//! Each test gets its own runtime directory, and therefore its own daemon and
//! sockets, so tests cannot see each other's agents.

use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

pub const AMON: &str = env!("CARGO_BIN_EXE_amon");

/// Long enough to absorb a slow machine, short enough that a genuine hang
/// fails the test rather than the suite timeout.
const DEADLINE: Duration = Duration::from_secs(10);

pub struct Sandbox {
    runtime: PathBuf,
}

impl Sandbox {
    pub fn new() -> Self {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        // Short by design: socket paths have to fit in sockaddr_un.
        let runtime = std::env::temp_dir().join(format!(
            "wl{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&runtime);
        std::fs::create_dir_all(&runtime).expect("runtime dir");
        Self { runtime }
    }

    pub fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(AMON);
        command.args(args);
        command.env("XDG_RUNTIME_DIR", &self.runtime);
        // Detection must see this build's bundled manifests, not whatever
        // remote manifest cache the developer's machine has accumulated.
        command.env("XDG_STATE_HOME", self.runtime.join("state"));
        // Desktop integrations write under HOME — Omarchy resolves its config
        // that way and ignores XDG_CONFIG_HOME — so a test must give them a
        // home of their own or it would reach the developer's real setup.
        command.env("HOME", self.runtime.join("home"));
        // Sandboxed too, so an inherited value cannot leak into anything else
        // that consults it — even though the Omarchy path deliberately does not.
        command.env("XDG_CONFIG_HOME", self.runtime.join("home/.config"));
        // The developer's machine may be an Omarchy box with a live shell, and
        // uninstalling the widget asks the `omarchy` CLI to restore the
        // built-in — reaching the real one would mutate the real bar. Constrain
        // PATH so external commands resolve to stubs planted in the sandbox
        // (via `fake_agent`) or, like Omarchy's own CLI dir, not at all.
        command.env("PATH", format!("{}:/usr/bin:/bin", self.runtime.display()));
        command
    }

    /// Where a desktop integration installs to, inside this sandbox.
    pub fn config_path(&self, relative: &str) -> PathBuf {
        self.home_path(".config").join(relative)
    }

    /// A path inside this sandbox's home directory.
    pub fn home_path(&self, relative: &str) -> PathBuf {
        self.runtime.join("home").join(relative)
    }

    pub fn run(&self, args: &[&str]) -> std::process::Output {
        self.command(args).output().expect("amon runs")
    }

    /// Starts an agent and leaves it running.
    pub fn spawn_agent(&self, argv: &[&str]) -> Child {
        self.command(argv)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("amon spawns")
    }

    pub fn status_json(&self) -> serde_json::Value {
        let output = self.run(&["status", "--json"]);
        let text = String::from_utf8_lossy(&output.stdout);
        serde_json::from_str(&text).unwrap_or(serde_json::Value::Array(vec![]))
    }

    /// Polls `amon status` until `predicate` accepts the agent list, and
    /// returns it. Panics with the last seen state, which is what you actually
    /// want to know when this fails.
    pub fn wait_for_status(
        &self,
        what: &str,
        predicate: impl Fn(&[serde_json::Value]) -> bool,
    ) -> Vec<serde_json::Value> {
        let start = Instant::now();
        let mut last = Vec::new();
        while start.elapsed() < DEADLINE {
            let value = self.status_json();
            last = value.as_array().cloned().unwrap_or_default();
            if predicate(&last) {
                return last;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        panic!("timed out waiting for {what}; last status was {last:#?}");
    }

    pub fn daemon_socket(&self) -> PathBuf {
        self.runtime.join("amon/amond.sock")
    }

    /// A path inside this sandbox's runtime directory — short enough for a
    /// unix socket, cleaned up with the sandbox.
    pub fn runtime_path(&self, name: &str) -> PathBuf {
        self.runtime.join(name)
    }

    pub fn runtime_dir(&self) -> &Path {
        &self.runtime
    }

    /// Connects to the daemon the way any other client would.
    pub fn connect(&self) -> UnixStream {
        let start = Instant::now();
        loop {
            if let Ok(stream) = UnixStream::connect(self.daemon_socket()) {
                stream
                    .set_read_timeout(Some(DEADLINE))
                    .expect("read timeout");
                return stream;
            }
            assert!(start.elapsed() < DEADLINE, "daemon never came up");
            std::thread::sleep(Duration::from_millis(25));
        }
    }

    /// Writes an executable script and returns its path. Naming it after a
    /// real agent is what makes amon apply that agent's detection manifest.
    pub fn fake_agent(&self, name: &str, script: &str) -> PathBuf {
        let path = self.runtime.join(name);
        std::fs::write(&path, script).expect("write fake agent");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
                .expect("make executable");
        }
        path
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        // Ask the daemon to exit rather than leaking one per test.
        if let Ok(mut stream) = UnixStream::connect(self.daemon_socket()) {
            let _ = stream.write_all(b"{\"id\":\"bye\",\"method\":\"daemon.shutdown\"}\n");
            let _ = stream.flush();
            let mut sink = String::new();
            let _ = stream.set_read_timeout(Some(Duration::from_secs(1)));
            let _ = BufReader::new(stream).read_line(&mut sink);
        }
        let _ = std::fs::remove_dir_all(&self.runtime);
    }
}

/// A raw protocol client, so tests exercise the same socket a third-party
/// subscriber would use.
pub struct Client {
    stream: UnixStream,
    reader: BufReader<UnixStream>,
}

impl Client {
    pub fn new(stream: UnixStream) -> Self {
        let reader = BufReader::new(stream.try_clone().expect("clone"));
        Self { stream, reader }
    }

    pub fn send(&mut self, line: &str) {
        self.stream.write_all(line.as_bytes()).expect("send");
        self.stream.write_all(b"\n").expect("newline");
        self.stream.flush().expect("flush");
    }

    pub fn read_frame(&mut self) -> serde_json::Value {
        let mut line = String::new();
        let read = self.reader.read_line(&mut line).expect("read frame");
        assert!(read > 0, "connection closed while waiting for a frame");
        serde_json::from_str(&line).expect("frame is json")
    }

    /// Reads frames until one is the named event, ignoring responses and
    /// events the test does not care about.
    pub fn wait_for_event(&mut self, event: &str) -> serde_json::Value {
        for _ in 0..64 {
            let frame = self.read_frame();
            if frame.get("event").and_then(|value| value.as_str()) == Some(event) {
                return frame;
            }
        }
        panic!("never saw event {event}");
    }
}

pub fn read_to_string(mut source: impl Read) -> String {
    let mut text = String::new();
    let _ = source.read_to_string(&mut text);
    text
}

pub fn agent_named(agents: &[serde_json::Value], name: &str) -> Option<serde_json::Value> {
    agents.iter().find(|agent| agent["agent"] == name).cloned()
}

pub fn state_of(agent: &serde_json::Value) -> &str {
    agent["state"].as_str().unwrap_or("")
}

pub fn path_str(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// Runs amon with a terminal for stdin but a pipe for stdout — `amon agent >
/// log` typed at a shell — and returns what landed in the pipe.
///
/// This split is the whole point: amon can see a terminal on one side and
/// still owe the other side nothing but the agent's bytes.
pub fn run_with_terminal_stdin(sandbox: &Sandbox, argv: &[&str]) -> Vec<u8> {
    use std::os::fd::{FromRawFd, RawFd};

    let (mut controller, mut terminal): (RawFd, RawFd) = (0, 0);
    let opened = unsafe {
        libc::openpty(
            &mut controller,
            &mut terminal,
            std::ptr::null_mut(),
            std::ptr::null(),
            std::ptr::null(),
        )
    };
    assert_eq!(opened, 0, "openpty");

    // The child owns the terminal end as its stdin; the controller end stays
    // open here so the pty does not hang up under it.
    let stdin = unsafe { Stdio::from_raw_fd(terminal) };
    let output = sandbox
        .command(argv)
        .stdin(stdin)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .expect("amon runs");
    unsafe { libc::close(controller) };
    output.stdout
}

/// An agent running behind amon on a real PTY, driven the way a terminal
/// drives one. Only a terminal gets focus reporting, so this is the only way
/// to exercise it.
pub struct PtySession {
    pub child: Box<dyn portable_pty::Child + Send + Sync>,
    writer: Box<dyn Write + Send>,
    output: std::sync::Arc<std::sync::Mutex<Vec<u8>>>,
}

impl PtySession {
    /// Starts `amon <argv>` on a PTY, draining its output in the background.
    pub fn start(sandbox: &Sandbox, argv: &[&str]) -> Self {
        let pty = portable_pty::native_pty_system()
            .openpty(portable_pty::PtySize {
                rows: 24,
                cols: 80,
                pixel_width: 0,
                pixel_height: 0,
            })
            .expect("openpty");

        let mut command = portable_pty::CommandBuilder::new(AMON);
        command.args(argv);
        command.env("XDG_RUNTIME_DIR", sandbox.runtime_dir());
        command.env("XDG_STATE_HOME", sandbox.runtime_path("state"));
        // A compositor lookup would reach past the sandbox to the developer's
        // own desktop, and these tests are about focus, not windows.
        command.env_remove("HYPRLAND_INSTANCE_SIGNATURE");
        let child = pty.slave.spawn_command(command).expect("spawn");
        drop(pty.slave);

        let mut reader = pty.master.try_clone_reader().expect("reader");
        let writer = pty.master.take_writer().expect("writer");
        let output = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let sink = output.clone();
        std::thread::spawn(move || {
            let mut buffer = [0u8; 8192];
            while let Ok(read) = reader.read(&mut buffer) {
                if read == 0 {
                    return;
                }
                sink.lock()
                    .expect("sink")
                    .extend_from_slice(&buffer[..read]);
            }
        });

        Self {
            child,
            writer,
            output,
        }
    }

    /// Sends bytes as the terminal would — keystrokes, or a focus report.
    pub fn send(&mut self, bytes: &[u8]) {
        self.writer.write_all(bytes).expect("write to pty");
        self.writer.flush().expect("flush pty");
    }

    pub fn output(&self) -> Vec<u8> {
        self.output.lock().expect("output").clone()
    }

    /// Waits for amon itself to exit, which it does after the agent does.
    pub fn wait(&mut self) {
        let _ = self.child.wait();
    }

    pub fn kill(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }

    /// Waits for the agent's output to contain `needle`, so tests never race
    /// the agent's own echo.
    pub fn wait_for_output(&self, needle: &[u8]) -> Vec<u8> {
        let start = Instant::now();
        loop {
            let seen = self.output();
            if seen.windows(needle.len()).any(|window| window == needle) {
                return seen;
            }
            assert!(
                start.elapsed() < DEADLINE,
                "timed out waiting for {:?}; saw {:?}",
                String::from_utf8_lossy(needle),
                String::from_utf8_lossy(&seen)
            );
            std::thread::sleep(Duration::from_millis(25));
        }
    }
}

/// Runs a program on a plain PTY, with no amon involved, and returns exactly
/// what the terminal produced. The reference for "amon changed nothing".
pub fn run_on_bare_pty(program: &str) -> Vec<u8> {
    let pty = portable_pty::native_pty_system()
        .openpty(portable_pty::PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        })
        .expect("openpty");
    let mut child = pty
        .slave
        .spawn_command(portable_pty::CommandBuilder::new(program))
        .expect("spawn");
    drop(pty.slave);

    let mut reader = pty.master.try_clone_reader().expect("reader");
    // Held open for the same reason the wrapper holds its writer: dropping it
    // would push a newline and EOF into the pty and change what we capture.
    let _writer = pty.master.take_writer().expect("writer");

    let mut output = Vec::new();
    let mut buffer = [0u8; 8192];
    while let Ok(read) = reader.read(&mut buffer) {
        if read == 0 {
            break;
        }
        output.extend_from_slice(&buffer[..read]);
    }
    let _ = child.wait();
    output
}
