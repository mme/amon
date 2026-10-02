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
    let agents = agents().unwrap_or_default();

    // Pressed again while already on one of this workspace's agents: go to the
    // next one there instead of landing on the same agent forever. The first
    // press - from anywhere else - still takes the neediest; the cycle only
    // starts once you are standing on one of them.
    if let Some(next) =
        active_window().and_then(|focused| next_in_cycle(&agents, workspace, &focused))
    {
        if go_to(next)? {
            return Ok(());
        }
    }

    // Resolved before anything is dispatched — see the module note on why the
    // order is the whole point.
    if let Some(agent) = neediest_on(&agents, workspace) {
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

/// Takes the user to `agent`, reporting whether anything moved.
///
/// The one place amon moves anyone to an agent — see the module note. Both
/// halves of the jump live here so no caller can do one without the other.
fn go_to(agent: &AgentEntry) -> Result<bool, Box<dyn std::error::Error>> {
    // Checked here rather than trusted from the caller: this interpolates
    // into a Lua expression, and the token arrives over a socket.
    let address = agent.window.as_deref().filter(|window| is_address(window));
    let moved = match address {
        Some(address) => dispatch(&format!(
            "hl.dsp.focus({{ window = \"address:0x{address}\" }})"
        ))?,
        None => false,
    };
    // A window that was there and is not now — closed in between, or the
    // compositor never knew it — ends the jump: the pane hop marks the agent
    // seen, and an agent nobody was taken to has not been seen.
    if address.is_some() && !moved {
        return Ok(false);
    }
    // Inside a runtime the window is only half the jump: every agent in a
    // session shares the client's terminal, and the pane this one lives in
    // still has to come to the front. With no window at all — over ssh, no
    // compositor — the hop is the whole jump, and the only part amon can make.
    let Some(runtime) = &agent.runtime else {
        return Ok(moved);
    };
    runtime_hop(runtime);
    Ok(true)
}

/// Every connected agent, or `None` when the daemon is absent or slow - which
/// the caller treats as "no agents", degrading to the plain workspace switch.
fn agents() -> Option<Vec<AgentEntry>> {
    let mut client = Client::connect_running(RESOLVE_TIMEOUT).ok()?;
    let result = client.request(Method::Status).ok()??;
    let status: StatusResult = serde_json::from_value(result).ok()?;
    Some(status.agents)
}

/// The agent on `workspace` that most wants a human, window and all.
///
/// `None` for every reason that is not "there is one to jump to": no daemon,
/// a slow one, no agent there, none that wants anything, or one the compositor
/// never gave a window (over ssh, inside a multiplexer). All of them mean the
/// same thing to the caller — go to the workspace and let Hyprland choose.
fn neediest_on(agents: &[AgentEntry], workspace: u32) -> Option<AgentEntry> {
    let workspace = workspace.to_string();
    agents
        .iter()
        .filter(|agent| agent.workspace.as_deref() == Some(workspace.as_str()))
        .filter(|agent| agent.wants_attention())
        .filter(|agent| agent.window.as_deref().is_some_and(is_address))
        // Ties broken by the older claim, which is how the registry orders the
        // same rank: of two agents equally blocked, the one that has been
        // waiting longer is the one to land on.
        .min_by_key(|agent| (agent.attention(), agent.state_since))
        .cloned()
}

/// The agent after the focused one on `workspace`, when the focused window is
/// one of that workspace's agents; `None` otherwise, which makes it a first
/// press.
///
/// The cycle runs in the panel's order - where the windows sit, left to right
/// then down, start time for the unplaced - and through every agent there, at
/// rest included, wrapping at the end. Not in urgency order: looking at a
/// finished agent marks it seen and drops its rank, and a cycle that re-ranked
/// under each press would skip agents or bounce between two.
fn next_in_cycle<'a>(
    agents: &'a [AgentEntry],
    workspace: u32,
    focused: &str,
) -> Option<&'a AgentEntry> {
    let workspace = workspace.to_string();
    let mut here: Vec<&AgentEntry> = agents
        .iter()
        .filter(|agent| agent.workspace.as_deref() == Some(workspace.as_str()))
        .filter(|agent| agent.window.as_deref().is_some_and(is_address))
        .collect();
    here.sort_by(|left, right| {
        let place = |agent: &AgentEntry| match agent.position {
            Some(position) => (0, Some(position)),
            None => (1, None),
        };
        place(left)
            .cmp(&place(right))
            .then_with(|| left.started_at.cmp(&right.started_at))
            .then_with(|| left.id.cmp(&right.id))
    });
    let current = here
        .iter()
        .position(|agent| agent.window.as_deref() == Some(focused))?;
    Some(here[(current + 1) % here.len()])
}

/// The focused window's address, normalized the way agents carry theirs.
fn active_window() -> Option<String> {
    let output = Command::new("hyprctl")
        .args(["activewindow", "-j"])
        .output()
        .ok()?;
    window_address(&String::from_utf8_lossy(&output.stdout))
}

/// The address out of `hyprctl activewindow -j`: `0x` stripped and lowercase,
/// as the wrapper stores an agent's window.
fn window_address(json: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(json).ok()?;
    let address = value.get("address")?.as_str()?;
    let normalized = address.trim_start_matches("0x").to_ascii_lowercase();
    is_address(&normalized).then_some(normalized)
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

    fn plain_entry() -> AgentEntry {
        AgentEntry {
            id: "x".into(),
            agent: "claude".into(),
            state: amon_protocol::AgentState::Idle,
            state_since: 0,
            cwd: String::new(),
            pid: 0,
            args: vec![],
            hostname: String::new(),
            started_at: 0,
            agent_session_id: None,
            agent_session_path: None,
            activity: None,
            window: None,
            position: None,
            workspace: None,
            project: None,
            subpath: None,
            branch: None,
            focused: None,
            seen: None,
            runtime: None,
        }
    }

    #[test]
    fn a_hosted_agent_without_a_window_still_gets_its_hop() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("luvus.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let served = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            let request: serde_json::Value = serde_json::from_str(&line).unwrap();
            let mut stream = stream;
            let _ = writeln!(stream, r#"{{"id":"r","result":{{"type":"ok"}}}}"#);
            request
        });

        let agent = AgentEntry {
            runtime: Some(Runtime::Luvus {
                socket: socket.to_string_lossy().into_owned(),
                session: None,
                pane: "7".into(),
            }),
            ..plain_entry()
        };
        assert!(go_to(&agent).unwrap(), "the hop is the whole jump here");
        let request = served.join().unwrap();
        assert_eq!(request["method"], "pane.focus");
        assert_eq!(request["params"]["pane"], "7");
    }

    #[test]
    fn a_plain_agent_without_a_window_is_nothing_to_jump_to() {
        assert!(!go_to(&plain_entry()).unwrap());
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

    fn agent(id: &str, workspace: &str, window: &str, x: i32) -> AgentEntry {
        let mut entry: AgentEntry = serde_json::from_value(serde_json::json!({
            "id": id, "agent": "claude", "state": "idle", "state_since": 1,
            "cwd": "/", "pid": 1, "args": [], "hostname": "h", "started_at": 1,
        }))
        .unwrap();
        entry.workspace = Some(workspace.into());
        entry.window = Some(window.into());
        entry.position = Some(amon_protocol::Position { x, y: 0 });
        entry
    }

    fn ids(next: Option<&AgentEntry>) -> Option<&str> {
        next.map(|agent| agent.id.as_str())
    }

    #[test]
    fn pressing_again_on_an_agent_goes_to_the_next_one_there_and_wraps() {
        // Listed out of order on purpose: the cycle follows where the windows
        // sit, left to right, like the panel - not the daemon's order.
        let agents = vec![
            agent("right", "2", "ccc", 900),
            agent("left", "2", "aaa", 10),
            agent("middle", "2", "bbb", 400),
            agent("elsewhere", "3", "ddd", 10),
        ];
        assert_eq!(ids(next_in_cycle(&agents, 2, "aaa")), Some("middle"));
        assert_eq!(ids(next_in_cycle(&agents, 2, "bbb")), Some("right"));
        assert_eq!(
            ids(next_in_cycle(&agents, 2, "ccc")),
            Some("left"),
            "wraps around"
        );
    }

    #[test]
    fn a_first_press_is_not_a_cycle() {
        let agents = vec![
            agent("left", "2", "aaa", 10),
            agent("right", "2", "ccc", 900),
        ];
        // Focus on another workspace's agent, or on a window that is no agent.
        assert!(next_in_cycle(&agents, 3, "aaa").is_none());
        assert!(next_in_cycle(&agents, 2, "browser").is_none());
    }

    #[test]
    fn a_lone_agent_cycles_to_itself() {
        let agents = vec![agent("only", "2", "aaa", 10)];
        assert_eq!(ids(next_in_cycle(&agents, 2, "aaa")), Some("only"));
    }

    #[test]
    fn the_active_window_address_is_read_the_way_agents_store_it() {
        assert_eq!(
            window_address(r#"{"address":"0x5643B0CB1810","class":"foot"}"#).as_deref(),
            Some("5643b0cb1810")
        );
        assert_eq!(window_address("{}"), None);
        assert_eq!(window_address("not json"), None);
    }
}
