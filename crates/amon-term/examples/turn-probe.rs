//! Where the submitted prompt sits relative to the narration, per frame.
//!
//! Two questions at once: can the prompt the agent is working on be read off
//! the screen, and does it arrive *before* the first narration line — which is
//! the window where a row today either says nothing or repeats the last turn.
use amon_detect::detect::manifest::{region, DetectionInput};
use amon_detect::identify_agent;
use amon_term::ShadowTerminal;
use regex::Regex;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args().nth(1).expect("usage: turn-probe <raw> <agent>");
    let label = std::env::args().nth(2).unwrap_or_else(|| "claude".into());
    let _ = identify_agent(&label).expect("known agent");
    let (spec, prompt_re, narration_re) = match label.as_str() {
        "codex" => ("before_current_prompt_marker", r"^\s*›\s+(?<t>\S.*?)\s*$", r"^\s*[•■✗✓]\s+(?<t>\S.*?)\s*$"),
        _ => ("above_prompt_box", r"^\s*❯\s+(?<t>\S.*?)\s*$", r"^\s*●\s+(?<t>\S.*?)\s*$"),
    };
    let prompt_re = Regex::new(prompt_re)?;
    let narration_re = Regex::new(narration_re)?;

    let bytes = std::fs::read(&path)?;
    let mut shadow = ShadowTerminal::new(100, 30)?;
    let mut last = String::new();
    let mut seen: Vec<(usize, String, String)> = Vec::new();
    let mut frame = 0;
    for chunk in bytes.chunks(256) {
        shadow.feed(chunk);
        let screen = shadow.detection_text();
        if screen == last { continue; }
        last = screen.clone();
        frame += 1;
        let input = DetectionInput { screen: &screen, osc_title: "", osc_progress: "" };
        let text = region(input, spec);
        let lines: Vec<&str> = text.lines().collect();

        let grab = |re: &Regex| -> Option<(usize, String)> {
            lines.iter().enumerate().rev().find_map(|(i, l)| {
                re.captures(l).map(|c| (i, c.name("t").unwrap().as_str().to_string()))
            })
        };
        let prompt = grab(&prompt_re);
        let narration = grab(&narration_re);
        let cur = match (&prompt, &narration) {
            (Some((pi, p)), Some((ni, n))) if ni > pi => ("narration", n.clone()),
            (Some((_, p)), _) => ("prompt", p.clone()),
            (None, Some((_, n))) => ("narration(no prompt)", n.clone()),
            _ => continue,
        };
        if seen.last().map(|(_, _, t)| t.as_str()) != Some(cur.1.as_str()) {
            seen.push((frame, cur.0.to_string(), cur.1.clone()));
        }
    }
    for (f, kind, text) in seen {
        let t: String = text.chars().take(62).collect();
        println!("  frame {f:>3}  {kind:<20} {t}");
    }
    Ok(())
}
