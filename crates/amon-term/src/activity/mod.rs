//! What the agent is doing, read off the rendered screen.
//!
//! Every column in a row says *where* an agent is. The Activity says what it
//! is busy with, in words that are never amon's own (ADR-0017, ADR-0020). It
//! comes in two kinds: a **Narration** — the line the harness draws to
//! narrate its step, `●` for Claude, a bullet for Codex — or the **Prompt**,
//! the submitted ask the current Turn is working on. Narration outranks
//! Prompt: what it is *doing* beats what was *asked*, whenever both exist.
//!
//! # The Turn
//!
//! The submitted prompt is the turn boundary. A Narration counts only if it
//! sits *after* the newest prompt on screen — one before it belongs to a
//! finished Turn, and showing it would be the row confidently narrating the
//! previous question (the exact failure ADR-0017 records). A Turn with no
//! Narration yet shows its Prompt, which is also what fills the thinking gap
//! between submitting and the first narrated step.
//!
//! # Nothing here knows about any particular agent
//!
//! Which lines to read is an entry in `carriers.toml`, not code: regions of
//! the screen and patterns that recognise the lines inside them. Adding an
//! agent is adding an entry. The regions are evaluated by herdr's accessor
//! (a seam, ADR-0021), so the structural knowledge — where a harness draws
//! its input box, how to cut the live prompt away from earlier ones — stays
//! in the vendored engine and keeps tracking it across re-vendors.
//!
//! Text the user is still typing can never be read back: every prompt region
//! in the table structurally excludes the live input line, so an unsubmitted
//! draft belongs to no Turn and appears nowhere.

use amon_detect::detect::manifest::{explain_with_input, region, DetectionInput};
use amon_detect::Agent;
use regex::Regex;

mod carriers;

/// The group a carrier pattern uses to say which part of the line to keep.
/// Without one, the whole match is kept.
const TEXT_GROUP: &str = "text";

/// Long enough for any row a panel would show, short enough that a runaway
/// line cannot travel over the wire. One bound for both kinds.
const MAX_CHARS: usize = 160;

/// What a screen (or, via [`ActivityTracker::begin_turn`], a hook) said the
/// agent is doing. The mirror of the wire's `Activity`, owned here so this
/// crate does not depend on the protocol crate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Activity {
    pub text: String,
    pub kind: ActivityKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActivityKind {
    Narration,
    Prompt,
}

/// Reads the Activity from successive frames, holding the last one it could
/// read and latching within a Turn.
///
/// The latch is the struct's whole reason to exist. Claude's narration marker
/// blinks while a tool call runs, and on the frames where it is absent the
/// prompt above it is the newest thing visible — without memory, the row
/// would flap between the ask and the doing several times a second. Once a
/// Turn has narrated, it never falls back to its Prompt; only the *next*
/// Turn's prompt moves the row back to a Prompt.
#[derive(Debug, Default)]
pub struct ActivityTracker {
    current: Option<Activity>,
    /// The Prompt that opened the current Turn, cleaned. What "same Turn"
    /// means on later frames.
    turn_prompt: Option<String>,
    /// Whether the current Turn was opened by a hook report. A hook carries
    /// the prompt exactly as typed; the screen shows a rendering of it —
    /// wrapped, possibly cut — so while a hook Turn is open, a screen prompt
    /// that reads as a rendering of the same ask must not replace the text.
    hook_turn: bool,
}

impl ActivityTracker {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn current(&self) -> Option<&Activity> {
        self.current.as_ref()
    }

    /// Forget everything, for when the session it described has ended: a
    /// `/clear`, a `--resume`, a new session id on the wire. Nothing on
    /// screen marks that boundary, so the caller has to say. Returns whether
    /// there was anything to forget.
    pub fn clear(&mut self) -> bool {
        self.turn_prompt = None;
        self.hook_turn = false;
        self.current.take().is_some()
    }

    /// A hook reported a submitted prompt: a Turn starts now, authoritatively
    /// — this is an event, where the screen only ever offers an inference.
    /// Returns whether [`ActivityTracker::current`] changed.
    pub fn begin_turn(&mut self, prompt: &str) -> bool {
        let Some(text) = clean(first_line(prompt).to_string()) else {
            return false;
        };
        self.turn_prompt = Some(text.clone());
        self.hook_turn = true;
        self.replace(Activity {
            text,
            kind: ActivityKind::Prompt,
        })
    }

    /// Read a frame. Returns whether [`ActivityTracker::current`] changed.
    pub fn observe(&mut self, agent: Agent, input: DetectionInput<'_>) -> bool {
        match read(agent, input) {
            None => false,
            Some(seen) => match seen.kind {
                ActivityKind::Narration => self.replace(seen),
                ActivityKind::Prompt => self.observe_prompt(seen),
            },
        }
    }

    fn observe_prompt(&mut self, seen: Activity) -> bool {
        if let Some(turn) = self.turn_prompt.as_deref() {
            let same_turn = if self.hook_turn {
                // The screen renders the hook's prompt wrapped to the
                // terminal, so its first line is a prefix of the exact text —
                // or equal, when the prompt was short.
                turn.starts_with(&seen.text) || seen.text.starts_with(turn)
            } else {
                turn == seen.text
            };
            if same_turn {
                // Same Turn, nothing new: either it has narrated and this
                // frame is the marker blinking off — not the Turn regressing
                // to its ask — or the row already shows this Turn's Prompt
                // (the hook's exact text, when a hook opened it).
                return false;
            }
        }
        // A different prompt: a new Turn, inferred from the screen.
        self.turn_prompt = Some(seen.text.clone());
        self.hook_turn = false;
        self.replace(seen)
    }

    fn replace(&mut self, activity: Activity) -> bool {
        if self.current.as_ref() == Some(&activity) {
            return false;
        }
        self.current = Some(activity);
        true
    }
}

/// The Activity one frame supports, or `None` if this frame does not say.
///
/// Separate from the tracker so the decision about one screen can be tested
/// against a captured frame without any history behind it. The tracker's
/// latch is what turns a per-frame answer into a stable row.
pub fn read(agent: Agent, input: DetectionInput<'_>) -> Option<Activity> {
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

    carriers::for_agent(agent).find_map(|carrier| carrier.read(input))
}

/// One entry of `carriers.toml`, with its patterns compiled.
#[derive(Debug)]
struct Carrier {
    gate_region: String,
    gate_pattern: Option<Regex>,
    region: String,
    pattern: Regex,
    reject: Option<Regex>,
    prompt_region: Option<String>,
    prompt_pattern: Option<Regex>,
    prompt_reject: Option<Regex>,
}

/// A matched line and where it starts in the whole screen — the coordinate
/// that lets a Narration and a Prompt found in *different* regions be ordered
/// against each other.
struct Found {
    offset: usize,
    text: String,
}

impl Carrier {
    fn read(&self, input: DetectionInput<'_>) -> Option<Activity> {
        if !self.shows_its_prompt(input) {
            return None;
        }
        let narration = self.newest(input, &self.region, &self.pattern, self.reject.as_ref());
        let prompt = self.prompt_region.as_deref().and_then(|spec| {
            let pattern = self.prompt_pattern.as_ref()?;
            self.newest(input, spec, pattern, self.prompt_reject.as_ref())
        });

        // The Turn rule: a Narration counts only after the newest prompt.
        // One before it narrates a finished Turn and must lose to the ask
        // the agent is actually working on.
        match (narration, prompt) {
            (Some(n), Some(p)) if n.offset > p.offset => Some(narration_of(n)),
            (_, Some(p)) => Some(Activity {
                text: p.text,
                kind: ActivityKind::Prompt,
            }),
            (Some(n), None) => Some(narration_of(n)),
            (None, None) => None,
        }
    }

    /// Whether the agent's own live input is on screen.
    ///
    /// An agent that is not showing its input is not showing its transcript
    /// either — it has handed the terminal to a pager, a diff viewer or an
    /// editor, and whatever is on screen belongs to that, not to the agent.
    /// What the input looks like is per-agent, which is why it is data:
    /// herdr's `prompt_box_body` finds Claude's box because Claude draws one;
    /// Codex's is a bare marker line, so its entry says what to look for.
    fn shows_its_prompt(&self, input: DetectionInput<'_>) -> bool {
        let text = region(input, &self.gate_region);
        match &self.gate_pattern {
            Some(pattern) => text.lines().any(|line| pattern.is_match(line)),
            None => !text.trim().is_empty(),
        }
    }

    /// The last line of `spec` that `pattern` matches and `reject` does not —
    /// last because the newest line is the current one — with its offset in
    /// the whole screen. Cleaned and bounded on the way out.
    fn newest(
        &self,
        input: DetectionInput<'_>,
        spec: &str,
        pattern: &Regex,
        reject: Option<&Regex>,
    ) -> Option<Found> {
        let slice = region(input, spec);
        slice.lines().rev().find_map(|line| {
            if reject.is_some_and(|it| it.is_match(line)) {
                return None;
            }
            let captures = pattern.captures(line)?;
            let kept = captures.name(TEXT_GROUP).or_else(|| captures.get(0))?;
            let text = clean(kept.as_str().to_string())?;
            Some(Found {
                offset: screen_offset(input.screen, line)?,
                text,
            })
        })
    }
}

fn narration_of(found: Found) -> Activity {
    Activity {
        text: found.text,
        kind: ActivityKind::Narration,
    }
}

/// Where `line` starts within `screen`, when `line` is a sub-slice of it —
/// which every line of every screen region is, since the regions borrow from
/// the screen. Pointer arithmetic rather than a search, because a line's
/// *content* can repeat and its position cannot.
fn screen_offset(screen: &str, line: &str) -> Option<usize> {
    let start = screen.as_ptr() as usize;
    let here = line.as_ptr() as usize;
    let offset = here.checked_sub(start)?;
    (offset <= screen.len()).then_some(offset)
}

fn first_line(text: &str) -> &str {
    text.lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("")
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
