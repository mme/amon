//! The shadow terminal: a headless copy of what the user is seeing.
//!
//! The wrapper hands the agent's output to the real terminal untouched and
//! feeds a copy in here. Nothing written to a shadow terminal is ever echoed
//! back to the agent — it answers no queries and emits no responses; the real
//! terminal remains the only thing the agent talks to.
//!
//! Emulation is Ghostty's, the same library herdr uses, so the vendored
//! detection manifests see the screen they were written against.

use libghostty_vt::fmt::Format;
use libghostty_vt::selection::{FormatOptions, Selection};
use libghostty_vt::terminal::{Mode, Options, Point, PointCoordinate, Terminal};

/// Scrollback kept behind the viewport. Detection only reads the last
/// screenful, but a little history costs nothing and keeps reflow honest.
const MAX_SCROLLBACK: usize = 10_000;

pub struct ShadowTerminal {
    terminal: Terminal<'static, 'static>,
    cols: u16,
    rows: u16,
}

impl ShadowTerminal {
    /// Creates a shadow sized to the real terminal. Callers must keep the two
    /// in step: detection regions are row-relative, so a shadow of the wrong
    /// size sees a different screen than the user does.
    pub fn new(cols: u16, rows: u16) -> Result<Self, libghostty_vt::Error> {
        let (cols, rows) = (cols.max(1), rows.max(1));
        let terminal = Terminal::new(Options {
            cols,
            rows,
            max_scrollback: MAX_SCROLLBACK,
        })?;
        Ok(Self {
            terminal,
            cols,
            rows,
        })
    }

    /// Feeds a copy of the agent's output. Never call this with anything the
    /// real terminal did not also receive.
    pub fn feed(&mut self, bytes: &[u8]) {
        self.terminal.vt_write(bytes);
    }

    pub fn resize(&mut self, cols: u16, rows: u16) -> Result<(), libghostty_vt::Error> {
        let (cols, rows) = (cols.max(1), rows.max(1));
        if (cols, rows) == (self.cols, self.rows) {
            return Ok(());
        }
        self.terminal.resize(cols, rows, 0, 0)?;
        self.cols = cols;
        self.rows = rows;
        Ok(())
    }

    pub fn size(&self) -> (u16, u16) {
        (self.cols, self.rows)
    }

    /// The title the agent set via OSC 0/2. Agents put useful live context
    /// here, and some manifest rules match on it directly.
    pub fn title(&self) -> Option<String> {
        match self.terminal.title() {
            Ok(title) if !title.is_empty() => Some(title.to_string()),
            _ => None,
        }
    }

    /// Whether the agent asked the terminal for focus events (DEC mode 1004).
    ///
    /// The wrapper enables 1004 itself to learn where the user is looking
    /// (ADR-0007), so this is what tells the two apart: when the agent wants
    /// focus events they are forwarded to it untouched, and when it does not,
    /// they are amon's own and get swallowed.
    pub fn agent_wants_focus_events(&self) -> bool {
        self.terminal.mode(Mode::FOCUS_EVENT).unwrap_or(false)
    }

    /// The text detection runs against: the last screenful of the buffer —
    /// one row per terminal row — read row by row, with trailing blank rows
    /// dropped. Manifest regions like `bottom_non_empty_lines(5)` are
    /// evaluated within this window.
    ///
    /// This reproduces herdr's `ghostty_detection_text` — same screen-height
    /// window, same per-row reads, same trailing newline — because the
    /// manifests were written against exactly that string.
    pub fn detection_text(&self) -> String {
        let lines = usize::from(self.rows).max(1);
        let Ok(total_rows) = self.terminal.total_rows() else {
            return String::new();
        };
        if total_rows == 0 || self.cols == 0 {
            return String::new();
        }

        let end = total_rows.saturating_sub(1);
        let start = end.saturating_add(1).saturating_sub(lines);
        let mut rows: Vec<String> = (start..=end).map(|y| self.row_text(y as u32)).collect();

        while rows.last().is_some_and(|row| row.trim().is_empty()) {
            rows.pop();
        }
        if rows.is_empty() {
            return String::new();
        }
        let start = rows.len().saturating_sub(lines);
        format!("{}\n", rows[start..].join("\n"))
    }

    fn row_text(&self, y: u32) -> String {
        let left = Point::Screen(PointCoordinate { x: 0, y });
        let right = Point::Screen(PointCoordinate {
            x: self.cols.saturating_sub(1),
            y,
        });
        let (Ok(start), Ok(end)) = (self.terminal.grid_ref(left), self.terminal.grid_ref(right))
        else {
            return String::new();
        };

        let selection = Selection::new(start, end, false);
        match self.terminal.format_selection_alloc(
            None,
            FormatOptions::new()
                .with_emit_format(Format::Plain)
                .with_selection(&selection)
                .with_trim(true),
        ) {
            Ok(Some(bytes)) => String::from_utf8_lossy(&bytes).trim_end().to_string(),
            _ => String::new(),
        }
    }
}
