//! The Super+N bindings' contract with a user's Hyprland config.
//!
//! The half amon owns outright may be rewritten freely; the half the user
//! wrote is the one these tests are really about. Every case here is a way of
//! asking the same question: did amon touch a line that was not its own?
//!
//! Each test gets its own `HOME`. Nextest gives every test its own process,
//! which is what makes setting it safe — the same reason the workspace uses
//! nextest at all (see the justfile).

use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};

use amon_integration::bindings;

struct Home {
    root: PathBuf,
}

impl Home {
    fn new() -> Self {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let root = std::env::temp_dir().join(format!(
            "amon-binds{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join(".config/hypr")).expect("hypr config dir");
        unsafe {
            std::env::set_var("HOME", &root);
        }
        Self { root }
    }

    fn theirs(&self) -> PathBuf {
        self.root.join(".config/hypr/bindings.lua")
    }

    fn ours(&self) -> PathBuf {
        self.root.join(".config/hypr/amon.lua")
    }

    fn write_theirs(&self, contents: &str) {
        std::fs::write(self.theirs(), contents).expect("write bindings.lua");
    }

    fn read_theirs(&self) -> String {
        std::fs::read_to_string(self.theirs()).unwrap_or_default()
    }
}

const THEIRS: &str = "-- my own bindings\no.bind(\"SUPER + SHIFT + R\", \"SSH\", \"alacritty\")\n";

#[test]
fn installing_writes_amons_own_file_and_one_line_of_the_users() {
    let home = Home::new();
    home.write_theirs(THEIRS);

    bindings::install().expect("install succeeds");

    // The half amon owns: the ten keys, bound by keycode the way Omarchy binds
    // them, so these replace those rather than sitting beside them.
    let ours = std::fs::read_to_string(home.ours()).expect("amon.lua written");
    assert!(ours.contains("code:"), "bound by keycode: {ours}");
    assert!(ours.contains("hl.unbind"), "unbound first: {ours}");
    assert!(ours.contains("focus "), "runs amon focus: {ours}");
    // Pinned to a real binary, because the compositor's PATH is not the
    // login shell's and a bare name could resolve to nothing.
    let binary = std::env::current_exe().expect("this test binary");
    assert!(
        ours.contains(binary.to_str().expect("utf-8 path")),
        "the absolute path is written in: {ours}"
    );

    // The half the user owns: their line intact, one require added.
    let theirs = home.read_theirs();
    assert!(theirs.contains("o.bind(\"SUPER + SHIFT + R\""), "{theirs}");
    assert_eq!(
        theirs.matches("require(\"hypr.amon\")").count(),
        1,
        "exactly one require: {theirs}"
    );
    assert!(bindings::installed(), "and both halves are seen as present");
}

#[test]
fn installing_twice_leaves_one_require() {
    let home = Home::new();
    home.write_theirs(THEIRS);

    bindings::install().expect("first");
    let once = home.read_theirs();
    bindings::install().expect("second");

    assert_eq!(home.read_theirs(), once, "a second setup changes nothing");
}

#[test]
fn removing_takes_both_halves_back_out() {
    let home = Home::new();
    home.write_theirs(THEIRS);
    bindings::install().expect("install");

    bindings::uninstall().expect("uninstall succeeds");

    assert_eq!(
        home.read_theirs(),
        THEIRS,
        "the user's file is byte-identical to before amon saw it"
    );
    assert!(!home.ours().exists(), "and amon's own file is gone");
    assert!(!bindings::installed());
}

#[test]
fn removing_what_was_never_installed_is_quiet() {
    let home = Home::new();
    home.write_theirs(THEIRS);

    let notes = bindings::uninstall().expect("uninstall succeeds");

    assert!(notes.is_empty(), "nothing to say: {notes:?}");
    assert_eq!(home.read_theirs(), THEIRS);
}

#[test]
fn a_truncated_fence_is_never_rewritten() {
    // A block missing its close makes everything after it look like amon's to
    // replace. Editing would erase the user's own bindings, so amon refuses
    // and says what to repair — the same rule the shell rc follows.
    let home = Home::new();
    let damaged = format!("-- >>> amon >>>\nrequire(\"hypr.amon\")\n{THEIRS}");
    home.write_theirs(&damaged);

    let notes = bindings::install().expect("install reports rather than fails");

    assert_eq!(
        home.read_theirs(),
        damaged,
        "a file amon cannot parse is a file amon does not touch"
    );
    assert!(
        notes
            .iter()
            .any(|note| note.contains("missing its closing")),
        "and the user is told what to repair: {notes:?}"
    );
    assert!(!home.ours().exists(), "nor is the other half written");
}

#[test]
fn a_symlinked_bindings_file_is_not_written_through() {
    // A dotfiles checkout linked into place: writing through the link would
    // put amon's line inside someone's repository.
    let home = Home::new();
    let real = home.root.join("dotfiles-bindings.lua");
    std::fs::write(&real, THEIRS).expect("the real file");
    std::os::unix::fs::symlink(&real, home.theirs()).expect("link it into place");

    let notes = bindings::install().expect("install reports rather than fails");

    assert_eq!(
        std::fs::read_to_string(&real).expect("still there"),
        THEIRS,
        "the file behind the link is untouched"
    );
    assert!(
        notes.iter().any(|note| note.contains("symlink")),
        "and the manual step is spelled out: {notes:?}"
    );
    assert!(
        notes
            .iter()
            .any(|note| note.contains("require(\"hypr.amon\")")),
        "including the line to add: {notes:?}"
    );
}
