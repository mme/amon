//! What the agent is doing, read off the rendered screen.
//!
//! Every column in a row says *where* an agent is. This says what it is busy
//! with, in the harness's own words — the line Claude prefixes with `●`. That
//! the wording is the harness's own is the point: a generic "Working…" is the
//! label ADR-0017 threw out, and the harness has already written a better one
//! for the human sitting in front of it.
//!
//! # Nothing here knows about any particular agent
//!
//! Which line to read is an entry in `carriers.toml`, not code: a region of
//! the screen, a pattern that recognises the line inside it, and how to tell
//! that the agent's own prompt is on screen at all. Adding an agent is adding
//! an entry.
//!
//! Neither half is amon's own invention. The region is evaluated by herdr's
//! accessor, so the structural knowledge — where a harness draws its input
//! box, what counts as a horizontal rule — stays in the vendored engine and
//! keeps tracking it across re-vendors. The pattern is an ordinary regex over
//! the lines that come back.
//!
//! # Why the carriers are not manifest rules
//!
//! They started as rules in the agent's manifest, which is the natural home
//! for "how this harness's screen is laid out", and the engine even reports
//! the text a rule looked at. Two things rule that out.
//!
//! The text comes back only as a 240-character head-anchored preview, and the
//! line worth reading sits at the *tail* of a window deep enough to clear the
//! input box — past 240 characters at any real terminal width.
//!
//! The bigger one is that a manifest is not amon's to extend. The daemon
//! fetches manifest updates, and they land per agent: a rule patched into the
//! bundled file is dropped by the next update to that agent, and a local
//! override carrying it shadows every future update instead
//! (`local_override_shadowing_remote`). Either way the feature and upstream's
//! detection improvements become each other's enemies. Keeping amon's data in
//! amon's own file leaves the manifests entirely upstream's, which is what
//! ADR-0001 vendored them for.
//!
//! What is emphatically *not* duplicated is logic: no region is
//! re-implemented, no screen is re-parsed, and whether a screen can be
//! believed at all is still herdr's verdict.

use amon_detect::detect::manifest::{explain_with_input, region, DetectionInput};
use amon_detect::Agent;
use regex::Regex;

mod carriers;

/// The group a carrier pattern uses to say which part of the line to keep.
/// Without one, the whole match is kept.
const TEXT_GROUP: &str = "text";

/// Long enough for any row a panel would show, short enough that a runaway
/// line cannot travel over the wire.
const MAX_CHARS: usize = 160;

/// Reads the activity line from successive frames, holding the last one it
/// could read.
///
/// Holding is the whole reason this is a struct rather than a function. The
/// marker blinks while a tool call runs, the screen is unreadable while a
/// viewer is open, and a redraw can be caught half-finished — on all of which
/// the honest answer is what it was doing a moment ago, not nothing. The value
/// is dropped only when the session it described is gone, which is something
/// only the caller can know.
#[derive(Debug, Default)]
pub struct ActivityTracker {
    current: Option<String>,
}

impl ActivityTracker {
    pub fn new() -> Self {
        Self::default()
    }

    /// What the agent is doing, as last read.
    pub fn current(&self) -> Option<&str> {
        self.current.as_deref()
    }

    /// Forget the current activity, for when the session it described has
    /// ended: a `/clear`, a `--resume`, a new session id on the wire. Nothing
    /// on screen marks that boundary, so the caller has to say. Returns
    /// whether there was anything to forget.
    pub fn clear(&mut self) -> bool {
        self.current.take().is_some()
    }

    /// Read a frame. Returns whether [`ActivityTracker::current`] changed.
    pub fn observe(&mut self, agent: Agent, input: DetectionInput<'_>) -> bool {
        let Some(text) = read(agent, input) else {
            return false;
        };
        if self.current.as_deref() == Some(text.as_str()) {
            return false;
        }
        self.current = Some(text);
        true
    }
}

/// The activity line on a single frame, or `None` if this frame does not say.
///
/// Separate from the tracker so that the decision about one screen can be
/// tested against a captured frame without any history behind it.
pub fn read(agent: Agent, input: DetectionInput<'_>) -> Option<String> {
    // herdr's own verdict that this screen is a viewer, a menu or a
    // half-drawn resize. It already suppresses state updates; suppressing
    // this too costs nothing and needs no second opinion.
    //
    // The observer computed this same flag a moment ago and could pass it in,
    // saving an evaluation. It does not, on purpose. An evaluation measures
    // 115µs against a 150ms frame — 0.08% of a core — and paying it keeps the
    // trust decision inside this function, where a test can hold a screen up
    // to it and prove it consults herdr rather than trusting a caller's word.
    if explain_with_input(agent, input).skip_state_update {
        return None;
    }

    carriers::for_agent(agent)
        .find_map(|carrier| carrier.read(input))
        .and_then(clean)
}

/// One entry of `carriers.toml`, with its patterns compiled.
#[derive(Debug)]
struct Carrier {
    prompt_region: String,
    prompt_pattern: Option<Regex>,
    region: String,
    pattern: Regex,
    reject: Option<Regex>,
}

impl Carrier {
    fn read(&self, input: DetectionInput<'_>) -> Option<String> {
        if !self.shows_its_prompt(input) {
            return None;
        }
        self.newest_line(input)
    }

    /// Whether the agent's own prompt is on screen.
    ///
    /// An agent that is not showing its prompt is not showing its transcript
    /// either — it has handed the terminal to a pager, a diff viewer or an
    /// editor, and whatever is on screen belongs to that, not to the agent.
    ///
    /// What the prompt looks like is per-agent, which is why it is data.
    /// herdr's `prompt_box_body` finds Claude's box because Claude draws one;
    /// on Codex it finds two rules around an *answer* and reports that as the
    /// box, so an agent whose prompt is a bare marker line has to say where to
    /// look and what to look for.
    fn shows_its_prompt(&self, input: DetectionInput<'_>) -> bool {
        let text = region(input, &self.prompt_region);
        match &self.prompt_pattern {
            Some(pattern) => text.lines().any(|line| pattern.is_match(line)),
            None => !text.trim().is_empty(),
        }
    }

    /// The last line of the carrier's region that its pattern matches and its
    /// reject pattern does not — last because the newest line is the current
    /// one, and the region reaches back far enough to hold more than one turn.
    fn newest_line(&self, input: DetectionInput<'_>) -> Option<String> {
        region(input, &self.region).lines().rev().find_map(|line| {
            if self.reject.as_ref().is_some_and(|it| it.is_match(line)) {
                return None;
            }
            let captures = self.pattern.captures(line)?;
            let kept = captures.name(TEXT_GROUP).or_else(|| captures.get(0))?;
            Some(kept.as_str().to_string())
        })
    }
}

/// Collapses the runs of spaces a terminal grid leaves behind, and bounds the
/// result. `None` for a line that turned out to hold no text.
fn clean(line: String) -> Option<String> {
    let mut cleaned = String::with_capacity(line.len());
    for word in line.split_whitespace() {
        if !cleaned.is_empty() {
            cleaned.push(' ');
        }
        cleaned.push_str(word);
    }
    if cleaned.is_empty() {
        return None;
    }
    if let Some((index, _)) = cleaned.char_indices().nth(MAX_CHARS) {
        cleaned.truncate(index);
        cleaned.push('…');
    }
    Some(cleaned)
}

#[cfg(test)]
mod tests;
