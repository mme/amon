// amon's own — no upstream counterpart.
//
// Subscribes to the amon daemon and answers one question for the bar: what is
// the most urgent agent state on a given workspace. Everything that talks to
// amon lives here so that the forked Workspaces.qml differs from Omarchy's by
// as little as possible (ADR-0008).

import QtQuick
import Quickshell
import Quickshell.Io

Item {
  id: root

  // Workspace name -> state string, for workspaces that have agents at all.
  // Workspaces absent from this map show nothing.
  property var stateByWorkspace: ({})

  // Agent id -> { workspace, state, seen }. The daemon sends whole entries,
  // and an entry disappears when its wrapper disconnects (ADR-0002).
  property var agents: ({})

  readonly property string socketPath: {
    const runtime = Quickshell.env("XDG_RUNTIME_DIR")
    // Without a runtime dir there is nothing to connect to; the daemon's
    // fallback location is per-uid and not derivable from here.
    return runtime && runtime.length > 0 ? runtime + "/amon/amond.sock" : ""
  }

  // Most urgent first. A finished agent nobody has looked at outranks one that
  // is still working: the dot exists to say "this wants you", and a working
  // agent wants nothing.
  readonly property var order: ["blocked", "done", "working", "idle"]

  function rank(state) {
    const at = root.order.indexOf(state)
    return at === -1 ? root.order.length : at
  }

  function recompute() {
    const next = ({})
    for (const id in root.agents) {
      const agent = root.agents[id]
      if (!agent.workspace) continue          // ssh, tmux, unmapped: not ours to place
      if (!agent.state) continue
      const previous = next[agent.workspace]
      if (previous === undefined || root.rank(agent.state) < root.rank(previous))
        next[agent.workspace] = agent.state
    }
    root.stateByWorkspace = next
  }

  // `unknown` is deliberately absent: an unrecognised process is usually not an
  // agent, and lighting the bar for it would cry wolf.
  function displayState(entry) {
    if (entry.state === "blocked") return "blocked"
    if (entry.state === "working") return "working"
    if (entry.state === "idle") return entry.seen === false ? "done" : "idle"
    return ""
  }

  function remember(entry) {
    if (!entry || !entry.id) return
    const state = root.displayState(entry)
    if (state === "") {
      delete root.agents[entry.id]
    } else {
      root.agents[entry.id] = { workspace: entry.workspace || "", state: state }
    }
    root.recompute()
  }

  function forget(id) {
    delete root.agents[id]
    root.recompute()
  }

  function clear() {
    root.agents = ({})
    root.stateByWorkspace = ({})
  }

  function handle(line) {
    let frame
    try {
      frame = JSON.parse(line)
    } catch (error) {
      return
    }

    // Events the daemon pushes.
    if (frame.event === "agent_connected" || frame.event === "agent_updated")
      return root.remember(frame.params)
    if (frame.event === "agent_disconnected")
      return root.forget(frame.params ? frame.params.id : "")

    // The reply to `status`, which seeds everything already running:
    // `subscribe` only registers for what happens next, so without this an
    // agent started before the bar came up would stay invisible.
    if (frame.id === "amon-status" && frame.result && frame.result.agents) {
      const agents = frame.result.agents
      for (let i = 0; i < agents.length; i++) root.remember(agents[i])
    }
  }

  Socket {
    id: sock
    path: root.socketPath

    // Set imperatively rather than bound: the shell starts at login, before
    // any daemon exists, so reconnecting is the ordinary path and not an edge
    // case. A binding here would be re-evaluated to its original value and
    // fight every attempt to reopen the socket.
    Component.onCompleted: if (root.socketPath !== "") sock.connected = true

    parser: SplitParser {
      splitMarker: "\n"
      onRead: line => root.handle(line)
    }

    onConnectionStateChanged: {
      if (sock.connected) {
        sock.write('{"id":"amon-hello","method":"hello","params":{"role":"subscriber","protocol":1,"version":"omarchy-widget"}}\n')
        // Subscribe first, then ask for the current set: the other order can
        // drop an agent that registers in between.
        sock.write('{"id":"amon-subscribe","method":"subscribe"}\n')
        sock.write('{"id":"amon-status","method":"status"}\n')
        sock.flush()
      } else {
        // Never show a stale dot: what amon can no longer see, the bar does
        // not claim to know.
        root.clear()
        retry.restart()
      }
    }
  }

  // Keeps trying for as long as there is no daemon. Never starts one.
  Timer {
    id: retry
    interval: 2000
    repeat: true
    running: !sock.connected && root.socketPath !== ""
    onTriggered: sock.connected = true
  }
}
