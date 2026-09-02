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

/// How many frames a hook Turn may hold for the screen to follow. A redraw is
/// one detection tick away, so this only has to outlast a slow one; past it
/// the hold is waiting on a divergence that will not come, and the screen is
/// the better guide. At the 150ms tick this is roughly a second.
const HOLD_FRAMES: u8 = 6;

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
    /// Whether a hook opened the current Turn — its prompt is the exact typed
    /// text, and the screen shows a wrapped rendering of it, so a screen prompt
    /// that is a prefix of the exact text is the same Turn and must not replace
    /// it. Independent of the hold below: true for the life of the hook Turn.
    hook_origin: bool,
    /// The previous Turn's prompt while a hook Turn waits for the screen to
    /// leave it. The hold releases when the screen stops showing this — a
    /// stronger signal than "matches the new prompt", which a prefix collision
    /// (old "Fix bug", new "Fix bug thoroughly") would answer wrongly.
    held_from: Option<String>,
    /// Frames the hold has been active. The hold bridges the one redraw
    /// between a hook firing and the screen following, so it is bounded: two
    /// prompts whose visible first row is identical (differing only in a
    /// wrapped continuation the prompt pattern never sees) are indistinguishable
    /// to the hold, and without a cap it would wait for a divergence that never
    /// comes. After the cap it trusts the screen.
    held_frames: u8,
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
        self.hook_origin = false;
        self.held_from = None;
        self.held_frames = 0;
        self.current.take().is_some()
    }

    /// A hook reported a submitted prompt: a Turn starts now, authoritatively
    /// — this is an event, where the screen only ever offers an inference.
    /// Returns whether [`ActivityTracker::current`] changed.
    pub fn begin_turn(&mut self, prompt: &str) -> bool {
        let Some(text) = clean(first_line(prompt).to_string()) else {
            return false;
        };
        let previous = self.turn_prompt.take();
        self.hook_origin = true;
        // Hold against the previous Turn while the screen still shows it, so
        // its stale narration cannot claim this ask. A re-submitted identical
        // prompt is held too — no prompt text can tell its new Turn from the
        // old one, so only the frame cap bridges it — but a first Turn, with
        // nothing before it, has nothing to wait out.
        self.held_from = previous;
        self.held_frames = 0;
        self.turn_prompt = Some(text.clone());
        self.replace(Activity {
            text,
            kind: ActivityKind::Prompt,
        })
    }

    /// A hook reported the harness's own account of its current step — for
    /// agents whose screens amon cannot read, this is the only Narration there
    /// is. It belongs to the current Turn and outranks the Prompt (kind first,
    /// as everywhere), and it releases any hold: a narrating turn is past the
    /// window the hold bridges. Returns whether
    /// [`ActivityTracker::current`] changed.
    pub fn hook_narration(&mut self, text: &str) -> bool {
        let Some(text) = clean(first_line(text).to_string()) else {
            return false;
        };
        self.held_from = None;
        self.held_frames = 0;
        self.replace(Activity {
            text,
            kind: ActivityKind::Narration,
        })
    }

    /// Read a frame. Returns whether [`ActivityTracker::current`] changed.
    pub fn observe(&mut self, agent: Agent, input: DetectionInput<'_>) -> bool {
        let Some(reading) = reading(agent, input) else {
            return false;
        };

        // A hook opened this Turn authoritatively, before the terminal had
        // redrawn. Until the screen catches up — until the prompt it shows is
        // the one the hook reported — anything on it belongs to the *previous*
        // Turn and must not replace the hook's Prompt. This is the guard that
        // keeps a stale narration from the turn before from clobbering the ask
        // the agent is now working on.
        // While a hook Turn waits for the screen, decide from the prompt on
        // screen whether the screen has left the previous Turn. The terminal
        // wraps a long prompt, so the screen shows a *prefix* of the exact
        // text — which makes both directions ambiguous:
        //   • a short old prompt can be a prefix of the new one
        //     ("Fix bug" → "Fix bug thoroughly"), and
        //   • a long new prompt shows only a prefix of itself.
        // The screen has caught up only when what it shows can be the new ask
        // and cannot merely be a truncation of the old one — released when the
        // shown text is the new prompt in full, or a prefix of it that is not
        // also a prefix of the previous. The ambiguous remainder holds, which
        // keeps the exact hook text on the row and self-corrects once the
        // screen diverges.
        if self.held_from.is_some() {
            let caught_up = reading.prompt.as_deref().is_some_and(|shown| {
                let new = self.turn_prompt.as_deref();
                let old = self.held_from.as_deref();
                // An identical re-submit shows the same prompt for both Turns,
                // so no frame is confidently the new one; the cap releases it.
                old != new
                    && (new == Some(shown)
                        || (new.is_some_and(|ask| ask.starts_with(shown))
                            && !old.is_some_and(|prev| prev.starts_with(shown))))
            });
            // A redraw is one tick away, so a hold that outlasts a handful of
            // them is not waiting on a redraw — it is the indistinguishable
            // case (identical visible first row). Trust the screen rather than
            // hold the row on the prompt forever.
            self.held_frames = self.held_frames.saturating_add(1);
            if !caught_up && self.held_frames <= HOLD_FRAMES {
                return false;
            }
            self.held_from = None;
        }
        let was_hook = self.hook_origin;

        match reading.activity.kind {
            // A Narration sits, by Carrier's rule, after the newest prompt —
            // so that prompt is the Turn it belongs to. Recording it keeps the
            // boundary current, which is what stops a later prompt-only frame
            // from reading as a regression rather than as a new Turn.
            ActivityKind::Narration => {
                if let Some(boundary) = reading.prompt {
                    self.turn_prompt = Some(boundary);
                }
                // Narration has taken over from the hook's Prompt, so the
                // exact-text preservation is done. Leaving hook_origin set
                // would keep prefix matching on into the next, screen-driven
                // Turn, where a prompt that is a prefix of this boundary would
                // be mistaken for it and the stale narration would stay.
                self.hook_origin = false;
                self.replace(reading.activity)
            }
            // A Prompt is either this Turn's ask still on screen (the marker
            // blinking off, or a hook Turn's exact text the screen is now
            // echoing) or the ask of a *new* Turn. Only the new Turn moves the
            // row; the same-Turn case keeps whatever is showing — which, for a
            // hook Turn, is the exact text, not the screen's wrapped rendering.
            ActivityKind::Prompt => {
                if self.matches_turn(&reading.activity.text, was_hook) {
                    return false;
                }
                self.hook_origin = false;
                self.turn_prompt = Some(reading.activity.text.clone());
                self.replace(reading.activity)
            }
        }
    }

    /// Whether a prompt seen on screen is the Turn already tracked. Exact
    /// normally; a prefix either way while a hook Turn's screen is catching
    /// up, since the terminal wraps the hook's exact text to its width.
    fn matches_turn(&self, shown: &str, wrapped: bool) -> bool {
        self.turn_prompt.as_deref().is_some_and(|turn| {
            if wrapped {
                turn.starts_with(shown) || shown.starts_with(turn)
            } else {
                turn == shown
            }
        })
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
    reading(agent, input).map(|r| r.activity)
}

/// One frame's Activity plus its turn boundary — what the tracker needs.
///
/// The trust gate lives here rather than being passed in: herdr's
/// `skip_state_update` is its own verdict that the screen is a viewer, a menu
/// or a half-drawn resize. The observer computed the same flag a moment ago
/// and could hand it over, but re-running it — 115µs against a 150ms frame,
/// 0.08% of a core — keeps the decision where a test can hold a screen up to
/// it and prove it consults herdr rather than trusting a caller's word.
fn reading(agent: Agent, input: DetectionInput<'_>) -> Option<Reading> {
    if explain_with_input(agent, input).skip_state_update {
        return None;
    }
    carriers::for_agent(agent).find_map(|carrier| carrier.reading(input))
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

/// What one frame supports: the Activity to show, and the submitted prompt
/// that opens the turn it belongs to. The prompt travels even when a Narration
/// wins, because the tracker needs the turn boundary to keep its latch honest
/// — without it, a narration that arrives in the same frame as a new prompt
/// would leave the boundary pointing at the previous turn.
struct Reading {
    activity: Activity,
    prompt: Option<String>,
}

impl Carrier {
    fn reading(&self, input: DetectionInput<'_>) -> Option<Reading> {
        if !self.shows_its_prompt(input) {
            return None;
        }
        let narration = self.newest(input, &self.region, &self.pattern, self.reject.as_ref());
        let prompt = self.prompt_region.as_deref().and_then(|spec| {
            let pattern = self.prompt_pattern.as_ref()?;
            self.newest(input, spec, pattern, self.prompt_reject.as_ref())
        });
        // The newest submitted prompt on screen is the turn boundary, carried
        // whichever kind wins.
        let boundary = prompt.as_ref().map(|found| found.text.clone());

        // The Turn rule: a Narration counts only after the newest prompt. One
        // before it narrates a finished Turn and must lose to the ask the
        // agent is actually working on.
        let activity = match (narration, prompt) {
            (Some(n), Some(p)) if n.offset > p.offset => narration_of(n),
            (_, Some(p)) => Activity {
                text: p.text,
                kind: ActivityKind::Prompt,
            },
            (Some(n), None) => narration_of(n),
            (None, None) => return None,
        };
        Some(Reading {
            activity,
            prompt: boundary,
        })
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
