//! Replays a captured stream and asks herdr's engine, at every frame, whether
//! the screen can be believed.
//!
//! Both answers come from the vendored engine rather than from anything amon
//! reimplements. `skip_state_update` is the viewer/menu flag the observer
//! already honours. Whether the agent's input box is on screen is decided by
//! the `prompt_box_body` region — herdr's own logic for finding the box, run
//! by herdr's own evaluator — which `explain_with_input` reports per rule,
//! matched or not, regardless of which rule won the state.
//!
//! That second point is the whole reason for using `explain` rather than
//! `detect`: `visible_idle` is gated on the *winning* rule and on the state
//! being Idle, so it goes false the moment the agent starts working even
//! though the box is still plainly there. The per-rule view has no such gate.

use amon_detect::detect::manifest::{explain_with_input, DetectionInput};
use amon_detect::{identify_agent, Agent};
use amon_term::ShadowTerminal;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args()
        .nth(1)
        .expect("usage: trust-gate <raw> <agent>");
    let label = std::env::args().nth(2).unwrap_or_else(|| "claude".into());
    let agent: Agent = identify_agent(&label).expect("known agent");
    let bytes = std::fs::read(&path)?;

    let mut shadow = ShadowTerminal::new(100, 30)?;
    let mut last = String::new();
    // Replayed in slices so the screen is examined as it evolves, not just at
    // the end: the interesting frames are the transient ones.
    for chunk in bytes.chunks(256) {
        shadow.feed(chunk);
        let screen = shadow.detection_text();
        if screen == last {
            continue;
        }
        last = screen.clone();

        let explain = explain_with_input(
            agent,
            DetectionInput {
                screen: &screen,
                osc_title: shadow.title().unwrap_or_default().as_str(),
                osc_progress: "",
            },
        );
        let box_present = explain
            .evaluated_rules
            .iter()
            .any(|rule| rule.region == "prompt_box_body" && rule.matched);
        let top = screen
            .lines()
            .rev()
            .find(|line| !line.trim().is_empty())
            .unwrap_or("");
        println!(
            "state={:<8} skip={:<5} box={:<5} | {}",
            format!("{:?}", explain.state),
            explain.skip_state_update,
            box_present,
            &top[..top.char_indices().nth(58).map_or(top.len(), |(i, _)| i)]
        );
    }
    Ok(())
}
