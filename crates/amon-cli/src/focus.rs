//! `amon focus <workspace>` — go to a workspace and land on its agent.
//!
//! Both ways of reaching a workspace route through here: the Super+N bindings
//! `amon setup` installs, and the bar widget's click. One implementation means
//! they cannot disagree about where you end up, and one ranking decides it —
//! [`AgentEntry::attention`], the same order the bar draws and `amon status`
//! sorts by.
//!
//! **One dispatch, never two.** Focusing a window switches to its workspace on
//! the way, so resolving the agent *first* and then issuing a single focus
//! leaves no moment where the wrong window is on screen. Switching first and
//! correcting afterwards would be visible on every jump — the agent's terminal
//! arriving a frame late, after whatever was focused there before.
//!
//! **The switch happens regardless.** Once amon owns Super+N, a daemon that is
//! stopped, wedged or mid-restart must not cost the user their ability to
//! change workspace. Every failure here degrades to the plain switch that
//! binding replaced.

use std::process::Command;
use std::time::Duration;

use amon_protocol::{AgentEntry, Method, StatusResult};

use crate::Client;

/// How long the daemon gets to answer before the jump goes without it.
///
/// This sits between a keypress and the screen changing. The daemon is a local
/// unix socket answering from memory, so a healthy one is orders of magnitude
/// inside this; anything approaching it is a daemon that is wedged or gone. A
/// workspace switch that stalls is a worse failure than one that lands on the
/// window the compositor would have picked anyway.
const RESOLVE_TIMEOUT: Duration = Duration::from_millis(150);

/// What `hyprctl dispatch` says when it did the thing.
///
/// Its exit status does not: a stale window address comes back `warning:
/// window not found` and still exits 0, which is exactly the case that has to
/// fall back to the workspace.
const DISPATCHED: &str = "ok";

pub fn run(workspace: u32) -> Result<(), Box<dyn std::error::Error>> {
    // Resolved before anything is dispatched — see the module note on why the
    // order is the whole point.
    if let Some(address) = agent_window_on(workspace) {
        if dispatch(&format!(
            "hl.dsp.focus({{ window = \"address:0x{address}\" }})"
        ))? {
            return Ok(());
        }
        // The agent was there when the daemon answered and its window is not
        // there now: closed in between, or the compositor never knew it. The
        // workspace is still where the user asked to go.
    }
    dispatch(&format!("hl.dsp.focus({{ workspace = \"{workspace}\" }})"))?;
    Ok(())
}

/// The window of the agent on `workspace` that most wants a human.
///
/// `None` for every reason that is not "there is one to jump to": no daemon,
/// a slow one, no agent there, none that wants anything, or one the compositor
/// never gave a window (over ssh, inside a multiplexer). All of them mean the
/// same thing to the caller — go to the workspace and let Hyprland choose.
fn agent_window_on(workspace: u32) -> Option<String> {
    let mut client = Client::connect_running(RESOLVE_TIMEOUT).ok()?;
    let result = client.request(Method::Status).ok()??;
    let status: StatusResult = serde_json::from_value(result).ok()?;
    let workspace = workspace.to_string();

    status
        .agents
        .into_iter()
        .filter(|agent| agent.workspace.as_deref() == Some(workspace.as_str()))
        .filter(|agent| agent.wants_attention())
        .filter(|agent| agent.window.as_deref().is_some_and(is_address))
        // Ties broken by the older claim, which is how the registry orders the
        // same rank: of two agents equally blocked, the one that has been
        // waiting longer is the one to land on.
        .min_by_key(|agent| (agent.attention(), agent.state_since))
        .and_then(|agent: AgentEntry| agent.window)
}

/// Whether a window token is one this is willing to paste into a Lua
/// expression.
///
/// The token is opaque by contract and arrives from a wrapper over the socket,
/// which is enough reason not to interpolate it unexamined: a quote or a brace
/// in there would end up as syntax rather than as an address. Hyprland's are
/// hex, and the wrapper already strips `0x` and lowercases.
fn is_address(token: &str) -> bool {
    !token.is_empty() && token.len() <= 32 && token.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// Runs one dispatch, reporting whether the compositor acted on it.
///
/// `Err` is reserved for not being able to ask at all — no `hyprctl`, no
/// compositor. A dispatch that ran and declined is `Ok(false)`, because the
/// caller has something better to do about that than fail.
fn dispatch(expression: &str) -> Result<bool, Box<dyn std::error::Error>> {
    let output = Command::new("hyprctl")
        .arg("dispatch")
        .arg(expression)
        .output()?;
    Ok(String::from_utf8_lossy(&output.stdout).trim() == DISPATCHED)
}
