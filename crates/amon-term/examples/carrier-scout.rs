//! What a harness offers the activity reader, before any carrier is written.
//!
//! Prints the two things a carrier entry depends on — whether herdr can find
//! the input box (the reader's trust gate) and what the region above it holds
//! — so the answer for a new harness is read off a captured session rather
//! than guessed at.
//!
//!   cargo run -p amon-term --example carrier-scout -- <raw> <agent>

use amon_detect::detect::manifest::{explain_with_input, region, DetectionInput};
use amon_detect::identify_agent;
use amon_term::ShadowTerminal;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args()
        .nth(1)
        .expect("usage: carrier-scout <raw> <agent>");
    let label = std::env::args().nth(2).unwrap_or_else(|| "claude".into());
    let agent = identify_agent(&label).expect("known agent");
    let bytes = std::fs::read(&path)?;

    let mut shadow = ShadowTerminal::new(100, 30)?;
    shadow.feed(&bytes);
    let screen = shadow.detection_text();
    let title = shadow.title().unwrap_or_default();
    let input = DetectionInput {
        screen: &screen,
        osc_title: &title,
        osc_progress: "",
    };

    let explain = explain_with_input(agent, input);
    let box_body = region(input, "prompt_box_body");
    let above = region(input, "above_prompt_box");

    println!("agent            {label}");
    println!("state            {:?}", explain.state);
    println!("screen trusted   {}", !explain.skip_state_update);
    println!("box found        {}", !box_body.trim().is_empty());
    println!("box body         {:?}", box_body.trim());
    println!(
        "above_prompt_box {} of {} screen lines\n",
        above.lines().filter(|l| !l.trim().is_empty()).count(),
        screen.lines().filter(|l| !l.trim().is_empty()).count()
    );
    println!("--- last 14 non-empty screen lines, newest last ---");
    let lines: Vec<&str> = screen.lines().filter(|l| !l.trim().is_empty()).collect();
    for line in lines.iter().rev().take(14).rev() {
        let head: String = line.chars().take(88).collect();
        println!("  {head}");
    }
    Ok(())
}
