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

  // Agent id -> { workspace, state }. The daemon sends whole entries, and an
  // entry disappears when its wrapper disconnects (ADR-0002).
  property var agents: ({})

  // Resolved the way the daemon resolves it: an explicit socket wins, then the
  // runtime directory. Without either there is nothing to connect to — the
  // daemon's last-resort path is per-uid and not derivable from QML.
  readonly property string socketPath: {
    const explicit = Quickshell.env("AMON_DAEMON_SOCKET")
    if (explicit && explicit.length > 0) return explicit
    const runtime = Quickshell.env("XDG_RUNTIME_DIR")
    return runtime && runtime.length > 0 ? runtime + "/amon/amond.sock" : ""
  }

  // Most urgent first, matching the order the registry itself sorts by.
  readonly property var order: ["blocked", "working", "done", "idle"]

  // A working agent shows the braille spinner, the same ten frames herdr runs
  // in its own UI and at the same 80ms. It is also what amon's detection keys
  // off: codex's manifest matches exactly these characters, and nine more
  // manifests — claude, amp, cursor, droid and the rest — accept any character
  // in the braille block as the sign of a working agent. So the bar shows back
  // the very thing it read.
  //
  // Icon glyphs were tried first and abandoned. A set only animates without
  // twitching if every frame has the same ink box, and the card suits do not:
  // spade, diamond and club match, but every heart in the font is a pixel
  // shorter, and no substitute exists — all 70 heart glyphs were measured, and
  // the only ones on the suits' grid are hearts inside a circle or a card.
  // Braille has no such problem: one cell, one advance width, dots switching on
  // and off inside it.
  readonly property var spinnerFrames: [
    "⠋", "⠙", "⠹", "⠸", "⠼",
    "⠴", "⠦", "⠧", "⠇", "⠏"
  ]

  // One tick for the whole bar, so two working workspaces spin in step instead
  // of drifting out of phase with each other.
  property int spinnerFrame: 0
  readonly property string spinner: root.spinnerFrames[root.spinnerFrame]
  readonly property bool anyWorking: {
    for (const workspace in root.stateByWorkspace)
      if (root.stateByWorkspace[workspace] === "working") return true
    return false
  }

  // Events that arrived before the seed. Applying them first and the seed
  // afterwards would let the snapshot resurrect an agent that has already gone,
  // or rewind one that has already moved on.
  property bool seeded: false
  property var pending: []

  function rank(state) {
    const at = root.order.indexOf(state)
    return at === -1 ? root.order.length : at
  }

  function recompute() {
    const next = ({})
    for (const id in root.agents) {
      const agent = root.agents[id]
      if (!agent.workspace) continue          // ssh, tmux, unmapped: not ours to place
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
    if (state === "") delete root.agents[entry.id]
    else root.agents[entry.id] = { workspace: entry.workspace || "", state: state }
  }

  function forget(id) {
    if (id) delete root.agents[id]
  }

  function reset() {
    root.agents = ({})
    root.stateByWorkspace = ({})
    root.seeded = false
    root.pending = []
  }

  function applyEvent(frame) {
    if (frame.event === "agent_connected" || frame.event === "agent_updated")
      root.remember(frame.params)
    else if (frame.event === "agent_disconnected")
      root.forget(frame.params ? frame.params.id : "")
  }

  // The reply to `status`, which seeds everything already running: `subscribe`
  // only registers for what happens next, so without this an agent started
  // before the bar came up would stay invisible.
  function seed(agents) {
    root.agents = ({})
    for (let i = 0; i < agents.length; i++) root.remember(agents[i])
    root.seeded = true
    const buffered = root.pending
    root.pending = []
    for (let i = 0; i < buffered.length; i++) root.applyEvent(buffered[i])
    root.recompute()
  }

  function handle(line) {
    let frame
    try {
      frame = JSON.parse(line)
    } catch (error) {
      return
    }

    if (frame.event !== undefined) {
      if (!root.seeded) {
        // Bounded: a daemon that never answers `status` must not grow this
        // without limit. Overflowing means the buffer can no longer be
        // replayed faithfully, so start the whole conversation again rather
        // than seed from a snapshot that later events would contradict.
        if (root.pending.length >= 512) {
          root.relink()
          return
        }
        root.pending.push(frame)
        return
      }
      root.applyEvent(frame)
      root.recompute()
      return
    }

    if (frame.id === "amon-status" && frame.result && frame.result.agents)
      root.seed(frame.result.agents)
  }

  // Recreated rather than reopened. Quickshell keeps its internal socket after
  // a failed connect, so assigning `connected` again does nothing: the object
  // itself has to go. The bar starts at login before any daemon exists, which
  // makes this the ordinary path rather than an edge case.
  Component {
    id: linkComponent

    Socket {
      path: root.socketPath
      connected: true

      parser: SplitParser {
        splitMarker: "\n"
        onRead: line => root.handle(line)
      }

      onConnectionStateChanged: {
        if (connected) {
          root.reset()
          write('{"id":"amon-hello","method":"hello","params":{"role":"subscriber","protocol":1,"version":"omarchy-widget"}}\n')
          // Subscribe first, then ask for the current set: the other order can
          // drop an agent that registers in between.
          write('{"id":"amon-subscribe","method":"subscribe"}\n')
          write('{"id":"amon-status","method":"status"}\n')
          flush()
        } else {
          // Never show a stale state: what amon can no longer see, the bar
          // does not claim to know, so the workspaces go back to numbers.
          root.reset()
        }
      }
    }
  }

  Loader {
    id: link
    active: root.socketPath !== ""
    sourceComponent: linkComponent
  }

  // Throwing the socket away and building a new one is the only way to retry;
  // it is also the simplest way to start over from a state that cannot be
  // trusted.
  function relink() {
    root.reset()
    link.active = false
    link.active = root.socketPath !== ""
  }

  // Only while something is actually working: an idle bar animates nothing.
  Timer {
    interval: 80 // herdr's own cadence
    repeat: true
    running: root.anyWorking
    onTriggered: root.spinnerFrame = (root.spinnerFrame + 1) % root.spinnerFrames.length
    onRunningChanged: if (!running) root.spinnerFrame = 0
  }

  // Keeps trying for as long as there is no daemon, and never starts one.
  // Rebuilding the loader is what actually retries; a failed Socket will not
  // reconnect in place.
  Timer {
    interval: 2000
    repeat: true
    running: root.socketPath !== ""
    onTriggered: if (!link.item || !link.item.connected) root.relink()
  }
}
