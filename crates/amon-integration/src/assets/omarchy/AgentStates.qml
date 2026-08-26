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

  // How many agents are in each state, across every workspace. The panel's
  // header reads this; the bar does not.
  //
  // Assigned rather than bound, because `agents` is mutated in place — a
  // `delete agents[id]` notifies nothing, so a binding over it would go stale
  // the first time an agent disconnected. Every path that touches `agents`
  // ends in `recompute`, which is what makes an assigned property honest here
  // where a derived one would not be.
  property var counts: root.emptyCounts()

  // The agents the pane lists, ordered so that each workspace's agents are
  // together and the most urgent of them is first. Assigned in `recompute` for
  // the same reason `counts` is: `agents` is mutated in place.
  property var rows: []

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

  // Set by the daemon's `update_available` event — sent when its daily check
  // finds a release newer than what is running, and replayed to subscribers
  // who connect after the fact. Bare versions ("0.2.0"), the way the wire
  // carries them; whoever draws them adds the "v".
  property string installedVersion: ""
  property string latestVersion: ""
  readonly property bool updateAvailable: root.latestVersion !== ""

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
  // Derived from the per-agent tally, never from the per-workspace maxima:
  // blocked outranks working there, so a workspace holding both would hide
  // its working agent — and the panel's spinners froze exactly while an
  // agent waited for input (#38). A working agent ticks the spinner
  // wherever it sits, whatever its neighbours are doing.
  readonly property bool anyWorking: (root.counts.working || 0) > 0

  // Which workspace the compositor says you are on. Set by the widget, which
  // is the only party that knows — this object talks to the daemon, and the
  // daemon has no opinion about where you are looking.
  property string focusedWorkspace: ""

  // And which window, in the daemon's own coinage: lowercase hex, no 0x —
  // the form `AgentEntry::window` carries, so `initialIndex` can compare it
  // to a row's token directly. Set by the host that has Hyprland in reach;
  // "" wherever none does, which only costs the pick its window precision.
  property string focusedWindow: ""

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
    const listed = []
    for (const id in root.agents) {
      const agent = root.agents[id]
      if (!agent.workspace) continue          // ssh, tmux, unmapped: not ours to place
      // Counted after that test, so the header counts exactly the agents the
      // desktop can show you. One over ssh or in a bare tmux session has no
      // workspace to switch to, and a figure you cannot act on is a figure
      // that only raises questions.
      tally[agent.state] += 1
      listed.push(agent)
      const previous = next[agent.workspace]
      if (previous === undefined || root.rank(agent.state) < root.rank(previous))
        next[agent.workspace] = agent.state
    }
    root.stateByWorkspace = next
    root.counts = tally
    listed.sort(root.compareRows)
    root.rows = listed
  }

  // Grouped by workspace, and within a workspace in the order they were
  // started — oldest first, so a workspace reads as you built it up and a new
  // agent appears at the bottom.
  //
  // Ordered by something that cannot change, deliberately. Sorting by state put
  // the agent that most wanted you at the top of its group, which sounds right
  // and reads badly: rows move while you are looking at them, and the row under
  // the cursor is not the row you were about to choose. What an agent is doing
  // is already said by its glyph, its word, and the count in the header — none
  // of which move.
  //
  // Workspaces sort as numbers where they look like numbers. Omarchy names them
  // "1".."10" by default but a named workspace is legal, and comparing those as
  // strings would put "10" before "2".
  function compareRows(left, right) {
    if (left.workspace !== right.workspace) {
      const a = Number(left.workspace)
      const b = Number(right.workspace)
      if (!isNaN(a) && !isNaN(b)) return a - b
      return left.workspace < right.workspace ? -1 : 1
    }
    if (left.startedAt !== right.startedAt) return left.startedAt - right.startedAt
    // Two agents started in the same millisecond is not a real case; this only
    // keeps the order total so the sort cannot wobble.
    return left.id < right.id ? -1 : (left.id > right.id ? 1 : 0)
  }

  // Where the pane's cursor starts when it opens: on the row `amon focus`
  // would land on, and never on the agent you are already looking at — you
  // opened the pane to go somewhere else.
  //
  // "Where you are" is the focused window when it belongs to a row, and the
  // focused workspace only when it does not. The distinction is what lets the
  // pick cycle at all: Enter focuses one agent's *window*, so two agents on
  // one workspace are different places to be — a workspace-only skip treats
  // them as one place, skips both, and every open lands on the same row.
  //
  // Above idle, the pick is the most urgent rank, and within a rank the agent
  // that has been in it longest — the same key focus.rs sorts by, and a test
  // holds the two together, because two copies of one rule drift. This half
  // deliberately does not cycle: while something needs input, every open
  // lands on it until its state changes, or the cursor would sit somewhere
  // else while the header says "1 NEEDS INPUT". The cycling people expect
  // still happens, driven by the states themselves — answering the blocked
  // agent unblocks it, visiting a done one marks it seen — so open → Enter
  // walks everything that wants you, most starved first, with no counter
  // anywhere.
  //
  // With nothing urgent anywhere else, there is no ranking left to apply, so
  // the pick rotates instead: the row after your workspace's rows, wrapping
  // past the end. Stateless on purpose — where the cursor lands is a function
  // of which workspace you are on, never of how often the pane has been
  // opened, so from any given workspace it always lands the same place.
  // `stateSince` cannot drive this half: visiting an idle agent changes
  // nothing about it, so a longest-idle pick oscillates between the two
  // oldest rows and the third is never landed on.
  function initialIndex() {
    const rows = root.rows
    if (rows.length === 0) return 0

    // Where you are, as precisely as the compositor can say it. A focused
    // window that is some row's window names one agent; a workspace only
    // names the group, so it stands in exactly when no agent's window is the
    // one focused — you are beside the agents, not at one of them.
    let mine = -1
    let byWindow = false
    for (let i = 0; i < rows.length; i++) {
      if (root.focusedWindow !== "" && rows[i].window === root.focusedWindow) {
        mine = i
        byWindow = true
      } else if (!byWindow && rows[i].workspace === root.focusedWorkspace) {
        // Rows are grouped by workspace, so this ends as the group's last row
        // — the place the rotation counts from.
        mine = i
      }
    }

    const idle = root.rank("idle")
    let pick = -1
    for (let i = 0; i < rows.length; i++) {
      // Skip where you are: one row when the focused window names it — the
      // blocked agent beside it is still a place worth going — and the whole
      // group when only the workspace is known.
      if (byWindow ? i === mine
                   : rows[i].workspace === root.focusedWorkspace) continue
      if (root.rank(rows[i].state) >= idle) continue
      if (pick === -1
          || root.rank(rows[i].state) < root.rank(rows[pick].state)
          || (root.rank(rows[i].state) === root.rank(rows[pick].state)
              && rows[i].stateSince < rows[pick].stateSince))
        pick = i
    }
    if (pick !== -1) return pick
    // The row after yours, wrapping — or the top, off every agent's window
    // and workspace, where "somewhere else" has nothing to be relative to.
    return mine === -1 ? 0 : (mine + 1) % rows.length
  }

  // The heading a row carries, or "" when the row above it already sits under
  // that heading. One flat list with headings folded into it, which is how
  // Omarchy's network panel separates known networks from the rest — and it is
  // why the list can stay a single ListView with one selection running through
  // it, rather than a column of lists with a cursor that has to cross between
  // them.
  function sectionTitle(index) {
    const rows = root.rows
    if (index < 0 || index >= rows.length) return ""
    if (index > 0 && rows[index - 1].workspace === rows[index].workspace) return ""
    return "WORKSPACE " + rows[index].workspace
  }

  // The shortest form that is still unambiguous, matching `age` in the CLI so
  // that a row here and a line of `amon status` never disagree about how long
  // an agent has been at something. A test holds the two together.
  function age(since, now) {
    const seconds = Math.max(0, Math.floor(((now || Date.now()) - since) / 1000))
    if (seconds < 60) return seconds + "s"
    if (seconds < 3600) return Math.floor(seconds / 60) + "m"
    return Math.floor(seconds / 3600) + "h"
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
      return
    }
    // The whole of what a row needs, copied out of the daemon's entry once. The
    // bar only ever wanted `workspace` and `state`; the pane draws the agent
    // itself, so it needs the rest — and taking it here means the pane never
    // touches the wire format.
    root.agents[entry.id] = {
      id: entry.id,
      agent: entry.agent || "",
      state: state,
      workspace: entry.workspace || "",
      cwd: entry.cwd || "",
      // What orders rows within a workspace. Never changes, which is the point.
      startedAt: entry.started_at || 0,
      // Absent outside a repository and on a detached HEAD, which the pane
      // draws as an empty column rather than as a placeholder.
      branch: entry.branch || "",
      stateSince: entry.state_since || 0,
      // Opaque, and handed back to the compositor rather than parsed (ADR-0011
      // and the note on AgentEntry::window). Absent off a supported compositor,
      // which is why every use of it is guarded.
      window: entry.window || ""
    }
  }

  function forget(id) {
    if (id) delete root.agents[id]
  }

  function reset() {
    root.agents = ({})
    root.stateByWorkspace = ({})
    root.counts = root.emptyCounts()
    root.rows = []
    root.seeded = false
    root.pending = []
    // Cleared with the link rather than kept: a daemon restarted after its
    // own upgrade replays the event if there is still anything to say, and a
    // kept answer would outlive the very upgrade it was asking for.
    root.installedVersion = ""
    root.latestVersion = ""
  }

  function applyEvent(frame) {
    if (frame.event === "agent_connected" || frame.event === "agent_updated")
      root.remember(frame.params)
    else if (frame.event === "agent_disconnected")
      root.forget(frame.params ? frame.params.id : "")
    else if (frame.event === "update_available" && frame.params) {
      root.installedVersion = frame.params.installed || ""
      root.latestVersion = frame.params.latest || ""
    }
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
