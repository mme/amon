//! `amon focus` — go to an agent: a workspace's neediest, or a named one.
//!
//! **Every way of reaching an agent routes through here**: the Super+N
//! bindings `amon setup` installs, the bar widget's click, and the agent
//! panel's pick. One implementation means they cannot disagree about where
//! you end up, and one ranking decides it — [`AgentEntry::attention`], the
//! same order the bar draws and `amon status` sorts by. A caller that
//! dispatched the compositor itself would be a second implementation, and
//! the one that forgot the runtime's pane hop is exactly how that goes wrong.
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

use amon_protocol::{AgentEntry, Method, Runtime, StatusResult};

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
    if let Some(agent) = agent_on(workspace) {
        if go_to(&agent)? {
            return Ok(());
        }
        // The agent was there when the daemon answered and its window is not
        // there now: closed in between, or the compositor never knew it. The
        // workspace is still where the user asked to go.
    }
    dispatch(&format!("hl.dsp.focus({{ workspace = \"{workspace}\" }})"))?;
    Ok(())
}

/// `amon focus --agent <id>` — go to one named agent, wherever it is.
///
/// What the panel calls once you pick a row. There is no workspace fallback
/// here: you asked for an agent, not a place, so an agent that has gone in
/// the meantime means the screen stays where it is rather than moving
/// somewhere you did not ask for.
pub fn agent(id: &str) -> Result<(), Box<dyn std::error::Error>> {
    if let Some(agent) = agent_with_id(id) {
        go_to(&agent)?;
    }
    Ok(())
}

/// Takes the user to `agent`, reporting whether the compositor went.
///
/// The one place amon moves anyone to an agent — see the module note. Both
/// halves of the jump live here so no caller can do one without the other.
fn go_to(agent: &AgentEntry) -> Result<bool, Box<dyn std::error::Error>> {
    // Checked here rather than trusted from the caller: this interpolates
    // into a Lua expression, and the token arrives over a socket.
    let Some(address) = agent.window.as_deref().filter(|window| is_address(window)) else {
        return Ok(false);
    };
    if !dispatch(&format!(
        "hl.dsp.focus({{ window = \"address:0x{address}\" }})"
    ))? {
        return Ok(false);
    }
    // Inside a runtime the window is only half the jump: every agent in a
    // session shares the client's terminal, and the pane this one lives in
    // still has to come to the front. Only after the window actually moved —
    // the hop marks the agent seen, and an agent nobody was taken to has not
    // been seen.
    if let Some(runtime) = &agent.runtime {
        runtime_hop(runtime);
    }
    Ok(true)
}

/// The agent on `workspace` that most wants a human, window and all.
///
/// `None` for every reason that is not "there is one to jump to": no daemon,
/// a slow one, no agent there, none that wants anything, or one the compositor
/// never gave a window (over ssh, inside a multiplexer). All of them mean the
/// same thing to the caller — go to the workspace and let Hyprland choose.
fn agent_on(workspace: u32) -> Option<AgentEntry> {
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
}

/// One agent by its registry id, if it is still connected.
fn agent_with_id(id: &str) -> Option<AgentEntry> {
    let mut client = Client::connect_running(RESOLVE_TIMEOUT).ok()?;
    let result = client.request(Method::Status).ok()??;
    let status: StatusResult = serde_json::from_value(result).ok()?;
    status.agents.into_iter().find(|agent| agent.id == id)
}

/// One focus request at the runtime, then done — best-effort and bounded,
/// because this sits between a keypress and the screen settling. Every
/// failure mode leaves the user exactly where the window dispatch put them,
/// which is already the right window.
fn runtime_hop(runtime: &Runtime) {
    use std::io::{BufRead, BufReader, Write};

    let Ok(stream) = std::os::unix::net::UnixStream::connect(runtime.socket()) else {
        return;
    };
    let _ = stream.set_read_timeout(Some(RESOLVE_TIMEOUT));
    let _ = stream.set_write_timeout(Some(RESOLVE_TIMEOUT));
    let Ok(mut writer) = stream.try_clone() else {
        return;
    };
    let (method, params) = runtime.focus_request();
    let line = serde_json::json!({"id": "amon-focus", "method": method, "params": params});
    if writeln!(writer, "{line}").is_err() {
        return;
    }
    // Read the reply so the request is not torn down mid-parse; what it says
    // changes nothing we could do better.
    let mut reply = String::new();
    let _ = BufReader::new(stream).read_line(&mut reply);
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixListener;

    #[test]
    fn the_hop_sends_the_runtimes_own_focus_call_for_the_entrys_pane() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("herdr.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let served = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            let request: serde_json::Value = serde_json::from_str(&line).unwrap();
            let mut stream = stream;
            let _ = writeln!(stream, r#"{{"id":"r","result":{{"type":"agent_focus"}}}}"#);
            request
        });

        runtime_hop(&Runtime::Herdr {
            socket: socket.to_string_lossy().into_owned(),
            session: None,
            pane: "w1:p2".into(),
        });

        let request = served.join().unwrap();
        assert_eq!(request["method"], "agent.focus");
        assert_eq!(request["params"]["target"], "w1:p2");
    }

    #[test]
    fn a_dead_socket_is_silently_nothing() {
        // The user asked for a workspace switch; the runtime being gone must
        // not turn that into an error or a hang.
        runtime_hop(&Runtime::Luvus {
            socket: "/nonexistent/luvus.sock".into(),
            session: None,
            pane: "7".into(),
        });
    }
}
