//! Replays a captured PTY stream and prints each row with its background
//! colour, to find the structure a harness expresses through style rather
//! than through text.
//!
//! pi marks nothing with a glyph: prompts, replies and tool output are told
//! apart purely by the block colour behind them. A text-only accessor sees
//! three indistinguishable indented lines, so a parser for it has to read the
//! cell attributes the emulator already keeps.
//!
//!   cargo run -p amon-term --example replay-styles -- /tmp/claude-1000/harness-pi.raw

use libghostty_vt::style::StyleColor;
use libghostty_vt::terminal::{Options, Point, PointCoordinate, Terminal};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args().nth(1).expect("usage: replay-styles <raw>");
    let bytes = std::fs::read(&path)?;
    let (cols, rows) = (100u16, 30u16);

    let mut terminal = Terminal::new(Options {
        cols,
        rows,
        max_scrollback: 10_000,
    })?;
    terminal.vt_write(&bytes);

    let total = terminal.total_rows()?;
    let start = total.saturating_sub(u32::from(rows) as usize);
    for y in start..total {
        // The row's background is whatever most of its cells agree on; a
        // block-coloured row is uniform, an unstyled one is uniformly unset.
        let mut bg = String::from("default");
        let mut text = String::new();
        for x in 0..cols {
            let point = Point::Screen(PointCoordinate { x, y: y as u32 });
            let Ok(cell) = terminal.grid_ref(point) else {
                continue;
            };
            if let Ok(style) = cell.style() {
                if x == 2 {
                    bg = match style.bg_color {
                        StyleColor::None => "default".into(),
                        StyleColor::Palette(i) => format!("palette:{i:?}"),
                        StyleColor::Rgb(c) => format!("#{:02x}{:02x}{:02x}", c.r, c.g, c.b),
                    };
                }
            }
            let mut buf = [' '; 8];
            if let Ok(n) = cell.graphemes(&mut buf) {
                text.extend(&buf[..n]);
            } else {
                text.push(' ');
            }
        }
        let text = text.trim_end();
        if text.is_empty() && bg == "default" {
            continue;
        }
        let cut = text.char_indices().nth(76).map_or(text.len(), |(i, _)| i);
        println!("{bg:>10}  |{}", &text[..cut]);
    }
    Ok(())
}
