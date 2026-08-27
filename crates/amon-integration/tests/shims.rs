//! The shim directory's contract: it holds exactly the agents you set up, and
//! removing an integration takes its shim with it (ADR-0013).
//!
//! Each test gets its own `HOME`. Nextest gives every test its own process,
//! which is what makes setting it safe.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};

use amon_integration::{shims, IntegrationTarget};

const CLAUDE: IntegrationTarget = IntegrationTarget::Claude;
const CODEX: IntegrationTarget = IntegrationTarget::Codex;

/// Holds the sandbox home for the length of a test. The path itself is only
/// read back through `shims::dir()`, which is the point: the tests exercise
/// where amon decides to put things, not where a test thinks it should.
struct Home;

impl Home {
    fn new() -> Self {
        Self::named("amon-shims")
    }

    /// A home whose own path carries glob characters, which the shim has to
    /// treat as literal text rather than as a pattern.
    fn globbish() -> Self {
        Self::named("amon-shims[1]")
    }

    fn named(prefix: &str) -> Self {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let root = std::env::temp_dir().join(format!(
            "{prefix}{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("home");
        unsafe {
            std::env::set_var("HOME", &root);
        }
        Self
    }

    fn shim(&self, name: &str) -> PathBuf {
        shims::dir().expect("a shim dir").join(name)
    }
}

#[test]
fn a_shim_is_installed_for_each_of_an_agents_names() {
    let home = Home::new();

    shims::install(CLAUDE).expect("install");

    let path = home.shim("claude");
    assert!(path.is_file(), "a shim for the agent's command");
    assert!(shims::is_installed(CLAUDE));
    assert_eq!(shims::installed(), vec!["claude".to_string()]);

    use std::os::unix::fs::PermissionsExt;
    let mode = std::fs::metadata(&path).expect("stat").permissions().mode();
    assert_eq!(mode & 0o111, 0o111, "a shim nothing can execute is no shim");
}

#[test]
fn the_shim_stops_itself_recursing_and_double_wrapping() {
    // Both guards, because they stop different things. Stripping the directory
    // is what keeps `amon claude` from finding this same file again forever;
    // the AMON_WRAPPER_PID check is what keeps two amon processes off one agent
    // when amon was invoked directly.
    let home = Home::new();
    shims::install(CLAUDE).expect("install");

    let body = std::fs::read_to_string(home.shim("claude")).expect("read");
    let dir = shims::dir().expect("dir");
    assert!(
        body.contains(dir.to_str().expect("utf-8")),
        "the shim must know its own directory to strip it: {body}"
    );
    assert!(body.contains("AMON_WRAPPER_PID"), "{body}");
    // Against the caller, not merely present: the marks reach every descendant
    // of a wrapped agent, and one of those is an agent to wrap, not amon's own
    // child (ADR-0016).
    assert!(
        body.contains("$PPID"),
        "the guard must compare the pid of whoever called this shim: {body}"
    );
    let guard = body.find("AMON_WRAPPER_PID").expect("guard");
    let wrap = body.rfind("exec ").expect("the wrapping exec");
    assert!(
        guard < wrap,
        "the already-wrapped check must come before wrapping again: {body}"
    );
}

#[test]
fn a_shim_strips_a_directory_whose_path_looks_like_a_pattern() {
    // `${PATH//:$dir:/:}` expands the directory into a *pattern*, not into
    // literal text. A home with glob characters in it — `[1]` matching the one
    // character `1` — therefore never matches its own path, the shim directory
    // survives on PATH, and the `exec` below finds this same file again. Not a
    // wrong agent: a process that execs itself forever and never starts one.
    //
    // Driven through the already-inside-amon branch, so the shim execs the
    // agent directly and what it resolves to is the whole answer.
    let _home = Home::globbish();
    shims::install(CLAUDE).expect("install");

    let dir = shims::dir().expect("dir");
    let bin = dir.parent().expect("share dir").join("bin");
    std::fs::create_dir_all(&bin).expect("bin");
    let real = bin.join("claude");
    std::fs::write(&real, "#!/bin/sh\necho \"reached the agent\"\n").expect("write");
    make_executable(&real);

    let mut child = std::process::Command::new(dir.join("claude"))
        .env("PATH", format!("{}:{}", dir.display(), bin.display()))
        // Makes the guard fire: the shim's parent is this test process.
        .env("AMON_WRAPPER_PID", std::process::id().to_string())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .expect("the shim runs");

    // Bounded, because the failure being guarded against is an exec loop that
    // never returns on its own.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    while std::time::Instant::now() < deadline {
        if child.try_wait().expect("wait").is_some() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    let finished = child.try_wait().expect("wait").is_some();
    let _ = child.kill();
    let output = child.wait_with_output().expect("output");

    assert!(
        finished,
        "the shim never reached an agent — it is execing itself: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("reached the agent"),
        "the real agent must be what the stripped PATH resolves to: {}",
        String::from_utf8_lossy(&output.stdout)
    );
}

fn make_executable(path: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;
    let mut permissions = std::fs::metadata(path).expect("stat").permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(path, permissions).expect("chmod");
}

#[test]
fn removing_an_integration_removes_its_shim() {
    // The case that matters most: unchecking a row, or `amon remove <agent>`,
    // must not leave the agent's name taken over for everything Omarchy
    // launches.
    let home = Home::new();
    shims::install(CLAUDE).expect("install");
    shims::install(CODEX).expect("install");

    shims::uninstall(CLAUDE).expect("uninstall");

    assert!(!home.shim("claude").exists(), "the one removed is gone");
    assert!(home.shim("codex").is_file(), "the other is untouched");
    assert!(!shims::is_installed(CLAUDE));
    assert!(shims::is_installed(CODEX));
}

#[test]
fn the_directory_goes_when_the_last_shim_does() {
    // A PATH entry pointing at a directory that no longer exists is harmless,
    // but leaving an empty one behind after a full removal is litter.
    let _home = Home::new();
    shims::install(CLAUDE).expect("install");
    let dir = shims::dir().expect("dir");
    assert!(dir.is_dir());

    shims::uninstall(CLAUDE).expect("uninstall");

    assert!(!dir.exists(), "nothing left, so nothing kept");
}

#[test]
fn removing_a_shim_that_was_never_there_is_quiet() {
    let _home = Home::new();
    shims::uninstall(CLAUDE).expect("uninstall succeeds");
}

#[test]
fn remove_all_sweeps_orphans_a_target_can_no_longer_name() {
    // A shim whose integration is already gone cannot be reached by removing
    // that target — there is no target left to ask. Sweeping is the only way,
    // and the same reason the alias block is swept.
    let home = Home::new();
    shims::install(CLAUDE).expect("install");
    std::fs::write(home.shim("some-departed-agent"), "#!/bin/sh\n").expect("orphan");

    let notes = shims::remove_all().expect("sweep");

    assert!(!shims::dir().expect("dir").exists(), "directory and all");
    assert!(
        notes.iter().any(|note| note.contains("claude")),
        "and it says what stopped being wrapped: {notes:?}"
    );
}

#[test]
fn sweeping_nothing_says_nothing() {
    let _home = Home::new();
    let notes = shims::remove_all().expect("sweep");
    assert!(notes.is_empty(), "{notes:?}");
}

#[test]
fn installing_twice_leaves_one_shim() {
    let _home = Home::new();
    shims::install(CLAUDE).expect("first");
    shims::install(CLAUDE).expect("second");
    assert_eq!(shims::installed(), vec!["claude".to_string()]);
}

#[test]
fn a_shim_whose_amon_is_gone_still_runs_the_agent() {
    // The failure that would matter most: amon deleted without `amon remove`
    // leaves shims on PATH ahead of every agent. If they insisted on running
    // amon, every set-up agent would stop starting — a far worse outcome than
    // simply losing the wrapping.
    let _home = Home::new();
    shims::install(CLAUDE).expect("install");
    let body = std::fs::read_to_string(shims::dir().expect("dir").join("claude")).expect("read");

    let guard = body
        .find("-x ")
        .expect("the shim must check amon is executable before running it");
    let wrap = body.rfind("exec ").expect("the wrapping exec");
    assert!(
        guard < wrap,
        "the check has to come before the exec it guards: {body}"
    );
}
