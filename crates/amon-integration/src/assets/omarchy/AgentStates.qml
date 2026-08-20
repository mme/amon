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

  // How many agents are in each state, across every workspace. The switcher's
  // header reads this; the bar does not.
  //
  // Assigned rather than bound, because `agents` is mutated in place — a
  // `delete agents[id]` notifies nothing, so a binding over it would go stale
  // the first time an agent disconnected. Every path that touches `agents`
  // ends in `recompute`, which is what makes an assigned property honest here
  // where a derived one would not be.
  property var counts: root.emptyCounts()

  function emptyCounts() {
    return ({ blocked: 0, done: 0, working: 0, idle: 0 })
  }

  // Resolved the way the daemon resolves it: an explicit socket wins, then the
  // runtime directory. Without either there is nothing to connect to — the
  // daemon's last-resort path is per-uid and not derivable from QML.
  //
  // `amon` and not `amon-dev`, deliberately: an installed widget belongs to an
  // installed release daemon. A debug build keeps its own runtime directory, so
  // pointing a bar at one means setting AMON_DAEMON_SOCKET.
  readonly property string socketPath: {
    const explicit = Quickshell.env("AMON_DAEMON_SOCKET")
    if (explicit && explicit.length > 0) return explicit
    const runtime = Quickshell.env("XDG_RUNTIME_DIR")
    return runtime && runtime.length > 0 ? runtime + "/amon/amond.sock" : ""
  }

  // Most urgent first — the same order `AgentEntry::attention` gives the
  // registry, `amon status` and `amon focus`, so what the bar draws for a
  // workspace and what Super+N lands you on there are never two answers.
  // `done` above `working` is the point of it: work in progress asks nothing
  // of anyone, work finished and unseen is what is waiting on you. A test
  // pins this array to the Rust ranking, because nothing else can.
  readonly property var order: ["blocked", "done", "working", "idle"]

  // A working agent shows a braille spinner: a two-dot bar rotating inside the
  // cell's two middle rows. Braille because it is what amon's detection reads —
  // nine manifests, claude and amp and cursor and droid among them, take any
  // character in the block as the sign of a working agent — so the bar answers
  // in the alphabet it listens to.
  //
  // Which rows is the whole of it. A cell is four rows tall, and herdr's own
  // ten frames (⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏, also the exact set codex's manifest matches) light
  // only dots 1-6: their ink fills the top three quarters, so the spinner rides
  // high above the digits beside it. Lighting all four rows centres it, but at
  // bar size draws a block heavy enough to read as a blob. Rows two and three
  // are centred by construction and half the height, and two dots of the four
  // are enough to read as a moving shape rather than a speck.
  //
  // Icon glyphs were tried first and abandoned. A set only animates without
  // twitching if every frame has the same ink box, and the card suits do not:
  // spade, diamond and club match, but every heart in the font is a pixel
  // shorter, and no substitute exists — all 70 heart glyphs were measured, and
  // the only ones on the suits' grid are hearts inside a circle or a card.
  // Braille has no such problem: one cell, one advance width, dots switching on
  // and off inside it.
  readonly property var defaultFrames: [
    "⠒", "⠰", "⠤", "⠆"
  ]

  // The user's configuration, as the daemon last read it (ADR-0012). Pushed
  // over this same socket, so the bar never learns where the file lives and
  // cannot disagree with the daemon about what it says.
  //
  // Defaults are *here* and not in the daemon: what the bar looks like is the
  // bar's business, and a daemon shipping its own copy of these glyphs would be
  // a second opinion about it. Configuration only ever overrides, which is also
  // why a bar with no daemon looks like a bar with no configuration.
  property var config: ({})

  // Absent means "unset", so every unset field falls through to the default
  // beside it. The daemon omits what the file does not say rather than sending
  // zeros, which is what makes a half-filled config file legible.
  function tuned(name, fallback) {
    const bar = root.config.bar
    const value = bar && bar[name]
    return (value === undefined || value === null) ? fallback : value
  }
  function glyph(name, fallback) {
    const glyphs = root.config.bar && root.config.bar.glyphs
    const value = glyphs && glyphs[name]
    return (value === undefined || value === null || value === "") ? fallback : value
  }

  readonly property var spinnerFrames: {
    const frames = root.glyph("working", null)
    return (frames && frames.length > 0) ? frames : root.defaultFrames
  }
  readonly property string blockedGlyph: root.glyph("blocked", String.fromCodePoint(0xF02D7))
  readonly property string doneGlyph: root.glyph("done", String.fromCodePoint(0xF05E0))
  readonly property string focusedGlyph: root.glyph("focused", String.fromCodePoint(0xF14FB))

  // One tick for the whole bar, so two working workspaces spin in step instead
  // of drifting out of phase with each other.
  property int spinnerFrame: 0
  readonly property string spinner: root.spinnerFrames[root.spinnerFrame]

  /// How long one frame is shown, and how long a whole turn therefore takes.
  readonly property int spinnerInterval: root.tuned("frame_ms", 200)
  readonly property int cycle: root.spinnerFrames.length * root.spinnerInterval

  /// The unit the focused workspace's turn-taking is counted in: half a turn.
  ///
  /// A whole turn was tried first and is the tidier idea — a state phase would
  /// be a whole number of revolutions — but every split it can express was
  /// either too brief to read or long enough to feel stuck, and the one that
  /// looked right on a real bar falls on a half. Derived from the cycle rather
  /// than written as 400 so the two rhythms stay related: change the spinner's
  /// speed and this follows it.
  readonly property int beat: root.cycle / 2

  /// The split, in beats — 2000ms showing the agent's state, 1200ms showing the
  /// marker. Named rather than written into the timer because these two numbers
  /// are the whole design, and they were settled by watching a bar rather than
  /// by argument: every ratio from 1:4 to 4:1 was tried, and the ones that read
  /// well all gave the state the longer half. You are already looking at the
  /// workspace you are on — the desktop is full of evidence for it — so the
  /// marker only has to confirm, while the state is the thing you cannot get
  /// any other way without leaving what you are doing.
  readonly property int stateBeats: root.tuned("state_beats", 5)
  readonly property int markerBeats: root.tuned("marker_beats", 3)
  readonly property bool anyWorking: {
    for (const workspace in root.stateByWorkspace)
      if (root.stateByWorkspace[workspace] === "working") return true
    return false
  }

  // Which workspace the compositor says you are on. Set by the widget, which
  // is the only party that knows — this object talks to the daemon, and the
  // daemon has no opinion about where you are looking.
  property string focusedWorkspace: ""

  /// Whether the focused workspace is holding something worth interrupting its
  /// marker for. An agent at rest is not: it asks for nothing, and the marker
  /// is the more useful thing to be showing.
  readonly property bool focusedWants: {
    const state = root.stateByWorkspace[root.focusedWorkspace] || ""
    return state !== "" && state !== "idle"
  }

  /// True while the focused workspace shows its agent's state rather than the
  /// focus marker. Global rather than per-workspace because only one workspace
  /// is ever focused, so one phase is the whole truth.
  property bool showingState: false

  /// Beats elapsed in the current phase, counted rather than timed.
  property int phaseBeats: 0

  /// Back to a full marker phase, from its first frame.
  ///
  /// Called on arrival as well as when the alternation starts, because moving
  /// between two workspaces that both hold busy agents never stops the timer —
  /// without this the new workspace would inherit the old one's place in the
  /// cycle and could land showing a glyph, which is indistinguishable from not
  /// having switched.
  function restartPhase() {
    root.showingState = false
    root.phaseBeats = 0
  }

  onFocusedWorkspaceChanged: root.restartPhase()

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
    const tally = root.emptyCounts()
    for (const id in root.agents) {
      const agent = root.agents[id]
      if (!agent.workspace) continue          // ssh, tmux, unmapped: not ours to place
      // Counted after that test, so the header counts exactly the agents the
      // desktop can show you. One over ssh or in a bare tmux session has no
      // workspace to switch to, and a figure you cannot act on is a figure
      // that only raises questions.
      tally[agent.state] += 1
      const previous = next[agent.workspace]
      if (previous === undefined || root.rank(agent.state) < root.rank(previous))
        next[agent.workspace] = agent.state
    }
    root.stateByWorkspace = next
    root.counts = tally
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
    root.counts = root.emptyCounts()
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
    // A daemon too old to know the method answers with an error and no result,
    // which lands here as "no configuration" — defaults, not a broken bar.
    if (frame.id === "amon-config" && frame.result && frame.result.config)
      root.config = frame.result.config
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
          write('{"id":"amon-config","method":"config"}\n')
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
    // Paced by the revolution, not by the frame: four frames at 200ms turn once
    // every 800ms, which is what herdr's ten at 80ms do. Keeping herdr's
    // interval instead would spin a quarter as many frames four times as fast.
    interval: 200
    repeat: true
    running: root.anyWorking
    onTriggered: root.spinnerFrame = (root.spinnerFrame + 1) % root.spinnerFrames.length
    onRunningChanged: if (!running) root.spinnerFrame = 0
  }

  // The focused workspace's turn-taking: one cycle of the agent's state, then
  // four of the marker, for as long as that agent wants something.
  //
  // The spinner above is deliberately not stopped while the marker is showing.
  // It is one tick for the whole bar, and pausing it would drift this
  // workspace out of step with every other working one — so a state phase
  // begins at whatever frame the bar is on and still runs exactly one
  // revolution, which is what the cycle is for.
  // Ticking once per cycle and counting, rather than setting the interval to
  // the length of each phase. Two reasons, both learned the hard way:
  //
  // `restart()` is `stop()` then `start()`, and both emit `runningChanged` —
  // so a handler that derives anything from `running` runs again mid-flip. The
  // first version toggled the phase and called `restart()`, and the `start()`
  // put the phase straight back, which made the marker phase last zero
  // milliseconds and the state show permanently. Imperatively starting a Timer
  // also destroys the binding on `running`, so it would have gone on ticking
  // after the workspace stopped wanting anything.
  //
  // A fixed interval needs neither, and counting beats is also what lets the
  // split fall on a half turn without the timer having to know that.
  Timer {
    interval: root.beat
    repeat: true
    running: root.focusedWants
    // Starting on the marker, and starting it over. The state leading was
    // tried first, on the reasoning that arriving somewhere should tell you
    // what is happening there — but arriving is itself the thing that needs
    // announcing. Landing mid-state-phase gives a switch no visual cue at all:
    // the workspace you left and the one you arrived at both just show agent
    // glyphs, and nothing says the focus moved. A full marker phase from the
    // first frame is that cue, and the state is a second away regardless.
    onRunningChanged: root.restartPhase()
    onTriggered: {
      root.phaseBeats += 1
      if (root.phaseBeats < (root.showingState ? root.stateBeats : root.markerBeats))
        return
      root.showingState = !root.showingState
      root.phaseBeats = 0
    }
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
