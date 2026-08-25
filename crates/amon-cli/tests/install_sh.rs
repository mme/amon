//! The curl installer, run for real against a mock of GitHub's release URLs.
//!
//! install.sh is the one piece of amon that executes on machines amon has
//! never touched, with nothing around it if it goes wrong — so it is tested
//! the way it runs: as `sh install.sh`, against a local server speaking the
//! three URLs the script knows (the /releases/latest redirect, the tarball,
//! the sha256 sidecar), into a scratch HOME. AMON_BASE_URL exists in the
//! script solely to point it here.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::path::Path;
use std::process::Command;
use std::sync::{Arc, Mutex};

const TAG: &str = "v9.9.9";

/// One fake release: the tarball publish.yml would build, and its sidecar.
struct Fixture {
    tarball: Vec<u8>,
    sha256_line: String,
}

fn fixture(dir: &Path) -> Fixture {
    std::fs::write(dir.join("amon"), "#!/bin/sh\necho fake amon\n").unwrap();
    std::fs::write(dir.join("LICENSE"), "Apache-2.0\n").unwrap();
    std::fs::write(dir.join("NOTICE"), "derived from herdr\n").unwrap();
    let name = format!("amon-{TAG}-x86_64-linux.tar.gz");
    assert!(Command::new("tar")
        .current_dir(dir)
        .args(["-czf", &name, "amon", "LICENSE", "NOTICE"])
        .status()
        .unwrap()
        .success());
    let out = Command::new("sha256sum")
        .current_dir(dir)
        .arg(&name)
        .output()
        .unwrap();
    Fixture {
        tarball: std::fs::read(dir.join(&name)).unwrap(),
        sha256_line: String::from_utf8(out.stdout).unwrap(),
    }
}

/// Serves the release URLs on an ephemeral port, recording every path hit.
/// `sha256_line` is a parameter so a test can serve a lying sidecar.
fn serve(fixture: Arc<Fixture>, sha256_line: String) -> (String, Arc<Mutex<Vec<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let hits = Arc::new(Mutex::new(Vec::new()));
    let log = hits.clone();
    let base_for_thread = base.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut line = String::new();
            if reader.read_line(&mut line).is_err() {
                continue;
            }
            let path = line.split_whitespace().nth(1).unwrap_or("").to_string();
            // drain headers; curl sends no body on GET/HEAD
            let mut header = String::new();
            while reader.read_line(&mut header).is_ok() && header.trim() != "" {
                header.clear();
            }
            log.lock().unwrap().push(path.clone());
            let tarball_path = format!("/releases/download/{TAG}/amon-{TAG}-x86_64-linux.tar.gz");
            let response: Vec<u8> = if path == "/releases/latest" {
                format!(
                    "HTTP/1.1 302 Found\r\nLocation: {base_for_thread}/releases/tag/{TAG}\r\nContent-Length: 0\r\n\r\n"
                )
                .into_bytes()
            } else if path == format!("/releases/tag/{TAG}") {
                b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n".to_vec()
            } else if path == tarball_path {
                let mut r = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n",
                    fixture.tarball.len()
                )
                .into_bytes();
                r.extend_from_slice(&fixture.tarball);
                r
            } else if path == format!("{tarball_path}.sha256") {
                let mut r = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n",
                    sha256_line.len()
                )
                .into_bytes();
                r.extend_from_slice(sha256_line.as_bytes());
                r
            } else {
                b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n".to_vec()
            };
            let _ = stream.write_all(&response);
            // read nothing more; curl reuses connections but a close is legal
        }
    });
    (base, hits)
}

struct Run {
    status: std::process::ExitStatus,
    stdout: String,
    stderr: String,
    home: std::path::PathBuf,
    scratch: std::path::PathBuf,
}

fn run_installer(base: &str, extra_env: &[(&str, &str)]) -> Run {
    let root = std::env::temp_dir().join(format!(
        "amon-install-test-{}-{}",
        std::process::id(),
        base.rsplit(':').next().unwrap()
    ));
    let _ = std::fs::remove_dir_all(&root);
    let home = root.join("home");
    let scratch = root.join("scratch");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&scratch).unwrap();
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../install.sh");
    let mut cmd = Command::new("sh");
    cmd.arg(script)
        .env("HOME", &home)
        .env("TMPDIR", &scratch)
        .env("AMON_BASE_URL", base);
    for (k, v) in extra_env {
        cmd.env(k, v);
    }
    let mut child = cmd
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdout = String::new();
    let mut stderr = String::new();
    child
        .stdout
        .take()
        .unwrap()
        .read_to_string(&mut stdout)
        .unwrap();
    child
        .stderr
        .take()
        .unwrap()
        .read_to_string(&mut stderr)
        .unwrap();
    Run {
        status: child.wait().unwrap(),
        stdout,
        stderr,
        home,
        scratch,
    }
}

fn scratch_is_clean(run: &Run) -> bool {
    std::fs::read_dir(&run.scratch).unwrap().next().is_none()
}

#[test]
fn installs_the_latest_release_where_omarchy_looks() {
    let dir = tempdir();
    let fx = Arc::new(fixture(&dir));
    let sha = fx.sha256_line.clone();
    let (base, hits) = serve(fx, sha);

    let run = run_installer(&base, &[]);

    assert!(run.status.success(), "stderr: {}", run.stderr);
    let binary = run.home.join(".local/bin/amon");
    assert!(binary.exists(), "the binary lands in ~/.local/bin");
    let mode =
        std::os::unix::fs::PermissionsExt::mode(&std::fs::metadata(&binary).unwrap().permissions());
    assert_eq!(mode & 0o111, 0o111, "and it is executable");
    assert!(run.home.join(".local/share/amon/LICENSE").exists());
    assert!(run.home.join(".local/share/amon/NOTICE").exists());
    assert!(
        hits.lock().unwrap().iter().any(|p| p == "/releases/latest"),
        "latest was resolved from the redirect"
    );
    assert!(run.stdout.contains("installed"), "stdout: {}", run.stdout);
    assert!(scratch_is_clean(&run), "the download scratch is cleaned up");
}

#[test]
fn a_pinned_version_never_asks_what_is_latest() {
    let dir = tempdir();
    let fx = Arc::new(fixture(&dir));
    let sha = fx.sha256_line.clone();
    let (base, hits) = serve(fx, sha);

    let run = run_installer(&base, &[("AMON_VERSION", TAG)]);

    assert!(run.status.success(), "stderr: {}", run.stderr);
    assert!(
        !hits.lock().unwrap().iter().any(|p| p == "/releases/latest"),
        "no latest lookup when the version is pinned"
    );
}

#[test]
fn a_wrong_checksum_installs_nothing_and_cleans_up() {
    let dir = tempdir();
    let fx = Arc::new(fixture(&dir));
    let lying = fx
        .sha256_line
        .replacen(|c: char| c.is_ascii_hexdigit(), "0", 4);
    let (base, _) = serve(fx, lying);

    let run = run_installer(&base, &[]);

    assert!(!run.status.success(), "a bad checksum must refuse");
    assert!(
        run.stderr.contains("checksum mismatch"),
        "stderr: {}",
        run.stderr
    );
    assert!(
        !run.home.join(".local/bin/amon").exists(),
        "nothing is installed on refusal"
    );
    assert!(
        scratch_is_clean(&run),
        "the refusal leaves no scratch behind"
    );
}

#[test]
fn a_foreign_architecture_is_turned_away_with_directions() {
    let dir = tempdir();
    let fx = Arc::new(fixture(&dir));
    let sha = fx.sha256_line.clone();
    let (base, hits) = serve(fx, sha);

    // A fake uname ahead of the real one is the only honest way to be armv7.
    let shims = dir.join("shims");
    std::fs::create_dir(&shims).unwrap();
    std::fs::write(
        shims.join("uname"),
        "#!/bin/sh\ncase \"$1\" in -m) echo armv7l;; *) echo Linux;; esac\n",
    )
    .unwrap();
    let mode = std::fs::Permissions::from_mode(0o755);
    std::fs::set_permissions(shims.join("uname"), mode).unwrap();
    let path = format!("{}:{}", shims.display(), std::env::var("PATH").unwrap());

    let run = run_installer(&base, &[("PATH", &path)]);

    assert!(!run.status.success());
    assert!(run.stderr.contains("x86_64 only"), "stderr: {}", run.stderr);
    assert!(
        run.stderr.contains("build from source"),
        "and points at the way that works"
    );
    assert!(
        hits.lock().unwrap().is_empty(),
        "nothing is downloaded first"
    );
}

#[test]
fn a_bin_dir_off_path_is_said_out_loud() {
    let dir = tempdir();
    let fx = Arc::new(fixture(&dir));
    let sha = fx.sha256_line.clone();
    let (base, _) = serve(fx, sha);

    let elsewhere = dir.join("elsewhere");
    let run = run_installer(&base, &[("AMON_PREFIX", elsewhere.to_str().unwrap())]);

    assert!(run.status.success(), "stderr: {}", run.stderr);
    assert!(elsewhere.join("amon").exists(), "AMON_PREFIX is honored");
    assert!(
        run.stdout.contains("not on PATH"),
        "the note fires when the directory is not on PATH: {}",
        run.stdout
    );
}

use std::os::unix::fs::PermissionsExt;

fn tempdir() -> std::path::PathBuf {
    static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let dir = std::env::temp_dir().join(format!(
        "amon-install-fx-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}
