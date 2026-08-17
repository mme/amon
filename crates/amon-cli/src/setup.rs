//! `amon setup` and `amon remove` — the interactive reconciler and its
//! non-interactive forms (ADR-0010).
//!
//! Bare `setup` on a TTY shows an inline checklist over the union of Detected
//! Agents and installed Integrations, plus the bar widget when the `omarchy`
//! CLI is around. Checked rows are set up or refreshed, unchecking an
//! installed row removes it, and Enter applies immediately — the screen is
//! the confirmation. Each action applies independently: one failure marks the
//! run failed but never holds the other rows hostage.

use std::io::{self, Read, Write};

use amon_integration::{alias, desktop, Candidate, DesktopTarget, InstallState};

/// Omarchy's own default-agent menu order (`omarchy-default-agent`'s supported
/// list), recorded as data rather than parsed out of their script at runtime.
/// It is their opinion on file; drift costs sort order only, never coverage —
/// agents Omarchy does not know simply sort after these.
const OMARCHY_MENU_ORDER: [&str; 9] = [
    "pi", "omp", "opencode", "claude", "codex", "grok", "gemini", "copilot", "crush",
];

/// The agent Omarchy launches by default, if the user chose one. Read straight
/// from the file `omarchy-default-agent` writes; the command itself prints
/// nothing when unset, and so does this.
fn default_agent() -> Option<String> {
    let home = std::env::var_os("HOME").filter(|home| !home.is_empty())?;
    let path = std::path::PathBuf::from(home).join(".config/omarchy/defaults/agent");
    let agent = std::fs::read_to_string(path).ok()?;
    let agent = agent.trim();
    (!agent.is_empty()).then(|| agent.to_string())
}

/// One row of the checklist.
struct Row {
    item: Item,
    checked: bool,
}

enum Item {
    Agent(Candidate),
    Widget { state: InstallState },
}

impl Row {
    fn label(&self) -> &str {
        match &self.item {
            Item::Agent(candidate) => candidate.label,
            Item::Widget { .. } => "Omarchy bar widget",
        }
    }

    fn state_text(&self, default: Option<&str>) -> String {
        let mut parts = Vec::new();
        match &self.item {
            Item::Agent(candidate) => {
                if default == Some(candidate.label) {
                    parts.push("default agent");
                }
                match candidate.state {
                    InstallState::NotInstalled => parts.push("detected"),
                    InstallState::Current => parts.push("installed"),
                    InstallState::Outdated => parts.push("outdated"),
                }
                if !candidate.detected {
                    parts.push("agent not found");
                }
            }
            Item::Widget { state } => match state {
                InstallState::NotInstalled => {}
                InstallState::Current => parts.push("installed"),
                InstallState::Outdated => parts.push("outdated"),
            },
        }
        parts.join(" · ")
    }

    fn installed(&self) -> bool {
        let state = match &self.item {
            Item::Agent(candidate) => candidate.state,
            Item::Widget { state } => *state,
        };
        state != InstallState::NotInstalled
    }
}

/// The rows, ordered: the system default agent first, then Omarchy's menu
/// order, then the rest alphabetically, the widget always last. Everything is
/// preselected — installed rows so re-confirming refreshes them, detected
/// rows because setting them up is what the screen is for.
fn rows() -> Vec<Row> {
    let default = default_agent();
    let mut agents = amon_integration::candidates();
    agents.sort_by_key(|candidate| {
        let position = OMARCHY_MENU_ORDER
            .iter()
            .position(|name| *name == candidate.label)
            .unwrap_or(OMARCHY_MENU_ORDER.len());
        (
            default.as_deref() != Some(candidate.label),
            position,
            candidate.label,
        )
    });

    let mut rows: Vec<Row> = agents
        .into_iter()
        .map(|candidate| Row {
            checked: true,
            item: Item::Agent(candidate),
        })
        .collect();

    let widget = desktop::status(DesktopTarget::Omarchy);
    if desktop::cli_present() || widget.state != amon_integration::InstallState::NotInstalled {
        rows.push(Row {
            checked: true,
            item: Item::Widget {
                state: widget.state,
            },
        });
    }
    rows
}

pub fn is_tty() -> bool {
    (unsafe { libc::isatty(0) } == 1) && (unsafe { libc::isatty(1) } == 1)
}

/// Bare `amon setup` on a terminal: the checklist, then the apply.
pub fn interactive() -> Result<(), Box<dyn std::error::Error>> {
    let mut rows = rows();
    if rows.is_empty() {
        print_empty_state();
        return Ok(());
    }

    let confirmed = pick(&mut rows)?;
    if !confirmed {
        return Ok(());
    }

    let mut actions = Vec::new();
    for row in &rows {
        match (&row.item, row.checked, row.installed()) {
            (Item::Agent(candidate), true, _) => actions.push(Action::SetupAgent {
                target: candidate.target,
                label: candidate.label,
                // Interactive setup always aliases: selecting an agent means
                // wrapping it (ADR-0009, as amended). The screen disclosed it.
                alias: true,
            }),
            (Item::Agent(candidate), false, true) => actions.push(Action::RemoveAgent {
                target: candidate.target,
                label: candidate.label,
            }),
            (Item::Widget { .. }, true, _) => actions.push(Action::SetupWidget),
            (Item::Widget { .. }, false, true) => actions.push(Action::RemoveWidget),
            (_, false, false) => {}
        }
    }
    apply(&actions)
}

/// `amon setup --all`: every Detected Agent plus the widget, no screen. Purely
/// additive — removal is the interactive screen's or `amon remove`'s job.
pub fn all(no_alias: bool) -> Result<(), Box<dyn std::error::Error>> {
    let mut actions: Vec<Action> = amon_integration::candidates()
        .into_iter()
        .filter(|candidate| candidate.detected)
        .map(|candidate| Action::SetupAgent {
            target: candidate.target,
            label: candidate.label,
            alias: !no_alias,
        })
        .collect();
    if desktop::cli_present() {
        actions.push(Action::SetupWidget);
    }
    if actions.is_empty() {
        print_empty_state();
        return Ok(());
    }
    apply(&actions)
}

/// `amon remove` with no target: list what is installed, confirm once, remove
/// everything. `--all` skips the confirmation for scripts.
pub fn remove_everything(assume_yes: bool) -> Result<(), Box<dyn std::error::Error>> {
    let mut actions = Vec::new();
    for status in amon_integration::statuses() {
        if status.state != InstallState::NotInstalled {
            actions.push(Action::RemoveAgent {
                target: status.target,
                label: status.label,
            });
        }
    }
    let widget_installed =
        desktop::status(DesktopTarget::Omarchy).state != InstallState::NotInstalled;
    if widget_installed {
        actions.push(Action::RemoveWidget);
    }
    // Last, so it sweeps up orphaned lines — aliases whose integration is
    // already gone would otherwise survive "remove everything" and keep
    // taking over their agent's name.
    if !alias::installed().is_empty() {
        actions.push(Action::ClearAliases);
    }

    if actions.is_empty() {
        println!("nothing to remove");
        return Ok(());
    }

    if !assume_yes {
        println!("this removes every amon integration:");
        for action in &actions {
            match action {
                Action::RemoveAgent { label, .. } => println!("  {label} — hooks and alias"),
                Action::RemoveWidget => println!("  Omarchy bar widget"),
                _ => {}
            }
        }
        for alias_line in alias::installed() {
            println!("  {alias_line}");
        }
        print!("remove everything? [y/N] ");
        io::stdout().flush()?;
        let mut answer = String::new();
        io::stdin().read_line(&mut answer)?;
        if !matches!(answer.trim(), "y" | "Y" | "yes") {
            println!("nothing removed");
            return Ok(());
        }
    }

    apply(&actions)
}

enum Action {
    SetupAgent {
        target: amon_integration::IntegrationTarget,
        label: &'static str,
        alias: bool,
    },
    RemoveAgent {
        target: amon_integration::IntegrationTarget,
        label: &'static str,
    },
    SetupWidget,
    RemoveWidget,
    /// Drops the whole bashrc block. Only `amon remove` with no target queues
    /// this, after every per-agent removal: it is what takes orphaned aliases
    /// — lines whose integration is already gone — with it.
    ClearAliases,
}

/// Runs every action independently, `✓`/`✗` per row, and ends on the first
/// success handoff. Failures make the exit non-zero but stop nothing — one
/// wedged integration must not hold the others hostage.
fn apply(actions: &[Action]) -> Result<(), Box<dyn std::error::Error>> {
    let mut failed = false;
    let mut first_command: Option<String> = None;

    for action in actions {
        match action {
            Action::SetupAgent {
                target,
                label,
                alias: with_alias,
            } => {
                let hooks = amon_integration::install(*target);
                let alias_notes = if *with_alias && hooks.is_ok() {
                    alias::install(*target)
                } else {
                    Ok(Vec::new())
                };
                match (hooks, alias_notes) {
                    (Ok(_), Ok(notes)) => {
                        // `install` returning Ok is not the same as the alias
                        // being live: a symlinked bashrc gets manual
                        // instructions instead of an edit. Verify, and pass
                        // those instructions on rather than claiming success.
                        let aliased = *with_alias && alias::is_installed(*target);
                        if aliased {
                            println!("✓ {label} — hooks installed, aliased");
                        } else if *with_alias {
                            println!("✓ {label} — hooks installed; the alias needs a manual step:");
                            for note in notes {
                                println!("    {note}");
                            }
                        } else {
                            println!("✓ {label} — hooks installed");
                        }
                        // The handoff must name what the user actually types:
                        // not always the label (cursor runs as cursor-agent),
                        // and without a live alias the bare name would launch
                        // the agent outside amon — the prefix is the truth.
                        let command = amon_integration::command_names(*target)
                            .first()
                            .copied()
                            .unwrap_or(label);
                        first_command.get_or_insert(if aliased {
                            command.to_string()
                        } else {
                            format!("amon {command}")
                        });
                    }
                    (Err(error), _) | (_, Err(error)) => {
                        failed = true;
                        println!("✗ {label} — {error}");
                    }
                }
            }
            Action::RemoveAgent { target, label } => {
                let hooks = amon_integration::uninstall(*target);
                let unaliased = alias::uninstall(*target);
                match (hooks, unaliased) {
                    // "unaliased" only when it is true: a block stranded in a
                    // symlinked bashrc survives `alias::uninstall`, which
                    // refuses to write through links.
                    (Ok(_), Ok(_)) => match alias::stranded() {
                        Some(note) => {
                            println!("✓ {label} — hooks removed");
                            println!("    {note}");
                        }
                        None => println!("✓ {label} — hooks removed, unaliased"),
                    },
                    (Err(error), _) | (_, Err(error)) => {
                        failed = true;
                        println!("✗ {label} — {error}");
                    }
                }
            }
            Action::SetupWidget => {
                let result = desktop::install(DesktopTarget::Omarchy)
                    .and_then(|_| desktop::enable(DesktopTarget::Omarchy));
                match result {
                    Ok(_) => println!("✓ bar widget — enabled on your bar"),
                    Err(error) => {
                        failed = true;
                        println!("✗ bar widget — {error}");
                    }
                }
            }
            Action::RemoveWidget => match desktop::uninstall(DesktopTarget::Omarchy) {
                Ok(notes) => {
                    // The notes say whether the built-in actually came back —
                    // a wedged `omarchy plugin disable` still lets removal
                    // proceed, with manual fix-up commands instead of a
                    // restore. Claiming "restored" here would bury them.
                    println!("✓ bar widget — removed");
                    for note in notes.iter().skip(1) {
                        println!("    {}", note.trim_start());
                    }
                }
                Err(error) => {
                    failed = true;
                    println!("✗ bar widget — {error}");
                }
            },
            Action::ClearAliases => match alias::remove_all() {
                Ok(notes) => {
                    for note in notes {
                        println!("✓ aliases — {note}");
                    }
                }
                Err(error) => {
                    failed = true;
                    println!("✗ aliases — {error}");
                }
            },
        }
    }

    if let Some(command) = first_command {
        println!();
        println!("open a new shell and run `{command}` —");
        println!("then `amon status` shows every agent.");
    }
    if failed {
        return Err("not everything succeeded (see above)".into());
    }
    Ok(())
}

/// The worst first experience — nothing detected — turned into orientation
/// instead of an empty checklist.
fn print_empty_state() {
    let supported: Vec<&str> = amon_integration::all_targets()
        .into_iter()
        .map(amon_integration::target_label)
        .collect();
    println!("no coding agents detected on this machine.");
    println!();
    println!("amon wraps: {}", supported.join(", "));
    println!("none of these are on your PATH right now.");
    println!();
    println!("`amon <agent>` works without any setup —");
    println!("hooks just add richer session data.");
}

// ---------------------------------------------------------------------------
// The inline checklist. Deliberately terminal-native: default foreground, dim
// for hints, one accent for the cursor and checkmarks, no alternate screen —
// output scrolls away like any other command's.
// ---------------------------------------------------------------------------

const DIM: &str = "\x1b[2m";
const ACCENT: &str = "\x1b[36m";
const RESET: &str = "\x1b[0m";

/// Runs the picker over `rows`, mutating their checked state. Returns whether
/// the user confirmed (Enter) rather than quit (q / Esc / Ctrl-C).
fn pick(rows: &mut [Row]) -> io::Result<bool> {
    let default = default_agent();
    let default = default.as_deref();
    let _raw = RawMode::enable()?;
    let mut out = io::stdout();
    let mut cursor = 0usize;

    write!(out, "\x1b[?25l")?; // hide the cursor; RawMode::drop restores it
    let lines = render(&mut out, rows, cursor, default, false)?;

    let confirmed = loop {
        match read_key()? {
            Key::Up => cursor = cursor.saturating_sub(1),
            Key::Down => cursor = (cursor + 1).min(rows.len() - 1),
            Key::Toggle => rows[cursor].checked = !rows[cursor].checked,
            Key::Enter => break true,
            Key::Quit => break false,
            Key::Other => {}
        }
        rewind(&mut out, lines)?;
        render(&mut out, rows, cursor, default, false)?;
    };

    // Leave the final state on screen, without the cursor marker, so what was
    // chosen stays readable above the apply report.
    rewind(&mut out, lines)?;
    render(&mut out, rows, cursor, default, true)?;
    Ok(confirmed)
}

/// Draws the whole screen and returns how many lines it took.
fn render(
    out: &mut impl Write,
    rows: &[Row],
    cursor: usize,
    default: Option<&str>,
    done: bool,
) -> io::Result<usize> {
    let width = rows.iter().map(|row| row.label().len()).max().unwrap_or(0);
    let mut lines = 0;

    writeln!(
        out,
        "Set up amon {DIM}— space toggles, enter applies{RESET}\r"
    )?;
    writeln!(out, "\r")?;
    lines += 2;
    for (index, row) in rows.iter().enumerate() {
        let pointer = if !done && index == cursor { "▸" } else { " " };
        let mark = if row.checked {
            format!("{ACCENT}[x]{RESET}")
        } else {
            "[ ]".to_string()
        };
        let state = row.state_text(default);
        writeln!(
            out,
            "{pointer} {mark} {:<width$}  {DIM}{state}{RESET}\r",
            row.label(),
        )?;
        lines += 1;
    }
    writeln!(out, "\r")?;
    writeln!(
        out,
        "{DIM}agents get session hooks and their name aliased\r"
    )?;
    writeln!(out, "to run under amon; the widget shows agent state\r")?;
    writeln!(out, "on your workspaces{RESET}\r")?;
    writeln!(out, "\r")?;
    writeln!(out, "{DIM}space toggle · enter apply · q quit{RESET}\r")?;
    lines += 6;
    out.flush()?;
    Ok(lines)
}

fn rewind(out: &mut impl Write, lines: usize) -> io::Result<()> {
    // Up and clear each line, so the redraw never leaves stale tails behind.
    for _ in 0..lines {
        write!(out, "\x1b[A\x1b[2K")?;
    }
    write!(out, "\r")?;
    Ok(())
}

enum Key {
    Up,
    Down,
    Toggle,
    Enter,
    Quit,
    Other,
}

fn read_key() -> io::Result<Key> {
    let mut byte = [0u8; 1];
    io::stdin().read_exact(&mut byte)?;
    Ok(match byte[0] {
        b' ' => Key::Toggle,
        b'\r' | b'\n' => Key::Enter,
        b'q' | 0x03 | 0x04 => Key::Quit, // q, Ctrl-C, Ctrl-D
        b'k' => Key::Up,
        b'j' => Key::Down,
        0x1b => {
            // Either a lone Esc (quit) or the start of an arrow sequence. The
            // two cannot be told apart without waiting, and waiting under
            // VMIN=1 blocks until the *next* keypress — a lone Esc would hang
            // the picker and then eat that key. So the continuation is read
            // under a deadline: an arrow's remaining bytes are already in
            // flight and arrive at once; silence means the Esc was alone.
            match read_escape_continuation()? {
                [b'[', b'A'] => Key::Up,
                [b'[', b'B'] => Key::Down,
                [0, 0] => Key::Quit,
                _ => Key::Other,
            }
        }
        _ => Key::Other,
    })
}

/// Reads up to two bytes following an Esc, giving up after a tenth of a
/// second (VMIN=0, VTIME=1) instead of blocking for a key that may never
/// come. Unread positions stay 0, which no continuation byte can be.
fn read_escape_continuation() -> io::Result<[u8; 2]> {
    let mut current: libc::termios = unsafe { std::mem::zeroed() };
    if unsafe { libc::tcgetattr(0, &mut current) } != 0 {
        return Err(io::Error::last_os_error());
    }
    let mut impatient = current;
    impatient.c_cc[libc::VMIN] = 0;
    impatient.c_cc[libc::VTIME] = 1;
    if unsafe { libc::tcsetattr(0, libc::TCSANOW, &impatient) } != 0 {
        return Err(io::Error::last_os_error());
    }

    let mut rest = [0u8; 2];
    let mut filled = 0;
    while filled < rest.len() {
        match io::stdin().read(&mut rest[filled..]) {
            Ok(0) => break,
            Ok(read) => filled += read,
            Err(error) => {
                unsafe { libc::tcsetattr(0, libc::TCSANOW, &current) };
                return Err(error);
            }
        }
    }

    if unsafe { libc::tcsetattr(0, libc::TCSANOW, &current) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(rest)
}

/// Puts stdin into raw-enough mode for single-key input and guarantees the
/// terminal comes back: echo, canonical mode, signals, and the cursor are all
/// restored on drop, however the picker ends.
struct RawMode {
    original: libc::termios,
}

impl RawMode {
    fn enable() -> io::Result<Self> {
        let mut original: libc::termios = unsafe { std::mem::zeroed() };
        if unsafe { libc::tcgetattr(0, &mut original) } != 0 {
            return Err(io::Error::last_os_error());
        }
        let mut raw = original;
        // ISIG off too: Ctrl-C is handled as quit so the drop below always
        // runs and the terminal is never left without echo or its cursor.
        raw.c_lflag &= !(libc::ICANON | libc::ECHO | libc::ISIG);
        raw.c_cc[libc::VMIN] = 1;
        raw.c_cc[libc::VTIME] = 0;
        if unsafe { libc::tcsetattr(0, libc::TCSANOW, &raw) } != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self { original })
    }
}

impl Drop for RawMode {
    fn drop(&mut self) {
        unsafe { libc::tcsetattr(0, libc::TCSANOW, &self.original) };
        let _ = write!(io::stdout(), "\x1b[?25h");
        let _ = io::stdout().flush();
    }
}
