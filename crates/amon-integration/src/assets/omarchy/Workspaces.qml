// Forked from Omarchy (ADR-0008). Do not edit the upstream parts by hand.
//
//   upstream: /usr/share/omarchy/shell/plugins/bar/widgets/Workspaces.qml
//   taken:    2026-11-08, Omarchy 4.5.0 (Hyprland 0.56.2, quickshell 0.3.2)
//
// Omarchy's bar has no way for one plugin to decorate another's widget, and
// the label a workspace shows is computed inside the widget that draws it:
// `modelData` is a `required property int`, and the widget reads no settings,
// so there is nowhere outside this file to say what a workspace displays.
// Hence a fork — upstream's widget with the label expression rewritten and
// nothing else changed, which is what keeps a re-sync a re-application rather
// than a merge.
//
// To re-derive: copy the upstream file over this one, then re-apply exactly
// the five changes, each marked `amon:` below —
//
//   1. `moduleName` names this plugin rather than Omarchy's
//   2. the `AgentStates` object
//   3. the `agentState` property, the `text` expression that reads it, and
//      the stand-in label that draws the spinner and an idle agent's number
//   4. `focusWorkspace` goes through `amon focus`
//   5. a click on the shown workspace acts on what it shows
//
// Anything else differing from upstream is drift, and the tests in desktop.rs
// exist to notice it.

import QtQuick
import QtQuick.Layouts
import Quickshell
import Quickshell.Hyprland
import Quickshell.Io
import qs.Commons
import qs.Ui

BarWidget {
  id: root
  moduleName: "sh.amon.workspaces"   // amon: upstream says omarchy.workspaces

  // amon: the one piece of state upstream does not have.
  //
  // Which workspace you are looking at is fed in rather than looked up: this
  // object holds the link to the daemon, and the daemon has no opinion about
  // where you are looking. Only the widget can say that — and since 4.5 each
  // bar says it for its own monitor, so a second screen takes turns for the
  // workspace it shows, not for the one the mouse is on.
  AgentStates {
    id: agents
    focusedWorkspace: root.activeWorkspaceId() > 0 ? String(root.activeWorkspaceId()) : ""
  }

  // Every monitor has workspaces 1-10 of its own, which Hyprland numbers in a
  // block of ten per monitor: 1-10, 11-20, and so on (see
  // default/hypr/monitor-workspaces.lua). Each bar shows its own monitor's block,
  // numbered 1-10, and the one that monitor shows, whichever monitor has focus.
  readonly property int slots: 10

  readonly property var monitor: {
    var window = root.QsWindow.window
    var screen = window ? window.screen : null
    return screen ? Hyprland.monitorFor(screen) : Hyprland.focusedMonitor
  }
  readonly property string monitorName: root.monitor ? root.monitor.name : ""

  // Connector -> block, from the file Hyprland keeps them in.
  property var blocks: ({})

  function readBlocks(text) {
    var next = {}
    var lines = String(text || "").split("\n")
    for (var i = 0; i < lines.length; i++) {
      var fields = lines[i].split("\t")
      if (fields.length >= 3 && fields[1] !== "") next[fields[1]] = parseInt(fields[0], 10)
    }
    root.blocks = next
  }

  FileView {
    path: (Quickshell.env("XDG_STATE_HOME") || Quickshell.env("HOME") + "/.local/state") + "/omarchy/monitor-workspaces"
    watchChanges: true
    printErrors: false
    onFileChanged: reload()
    onLoaded: root.readBlocks(text())
    onLoadFailed: root.readBlocks("")
  }

  function blockOf(id) {
    return Math.floor((id - 1) / root.slots)
  }

  function slotOf(id) {
    return (id - 1) % root.slots + 1
  }

  function activeWorkspaceId() {
    return root.monitor && root.monitor.activeWorkspace ? root.monitor.activeWorkspace.id : -1
  }

  // Until Hyprland has written the file, the workspace a monitor shows is in
  // its own block.
  readonly property int block: {
    var block = root.blocks[root.monitorName]
    if (block !== undefined) return block
    var active = root.activeWorkspaceId()
    return active > 0 ? root.blockOf(active) : 0
  }

  function workspaceById(id) {
    var values = Hyprland.workspaces.values
    for (var i = 0; i < values.length; i++) {
      if (values[i].id === id) return values[i]
    }

    return null
  }

  // The first five of the monitor's own, and any of the rest that exist. After
  // them come workspaces that are only visiting: those of a monitor that is
  // unplugged, which had no room to join this one's.
  function workspaceIds() {
    var base = root.block * root.slots
    var ids = [base + 1, base + 2, base + 3, base + 4, base + 5]
    var guests = []
    var values = Hyprland.workspaces.values

    for (var i = 0; i < values.length; i++) {
      var id = values[i].id
      if (id <= 0) continue
      if (root.blockOf(id) === root.block) {
        if (ids.indexOf(id) === -1) ids.push(id)
      } else if (values[i].monitor && values[i].monitor.name === root.monitorName) {
        guests.push(id)
      }
    }

    ids.sort(function(left, right) { return left - right })
    guests.sort(function(left, right) { return left - right })
    return ids.concat(guests)
  }

  function dispatch(expression) {
    if (root.bar) root.bar.run("hyprctl dispatch " + Util.shellQuote(expression))
  }

  // amon: clicking a workspace lands where Super+N lands — on the agent that
  // wants a human, not on whatever was focused there last. One command decides
  // that for both, so the two cannot drift apart. Upstream's two dispatches
  // become the fallback of one call each, and nothing else here changes.
  //
  // The `||` is the whole safety net: `bar.run` is `bash -lc`, so if amon is
  // not installed or not on the login shell's PATH, the click still switches
  // workspace exactly as upstream does — the expression after it is upstream's
  // own, unchanged. `id` is the model's int, so there is nothing to quote in
  // it.
  function focusWorkspace(id) {
    if (root.blockOf(id) === root.block) {
      root.focusViaAmon(id, "o.workspaces.focus(" + root.slotOf(id) + ", \"" + root.monitorName + "\")")
    } else {
      root.focusViaAmon(id, "hl.dsp.focus({ workspace = \"" + id + "\" })")
    }
  }
  // Kept beside the function it serves, with no blank line between: the drift
  // test reads a marked change as everything down to the next blank line.
  function focusViaAmon(id, expression) {
    if (root.bar) root.bar.run("amon focus " + id + " || hyprctl dispatch " + Util.shellQuote(expression))
  }

  // A wheel notch moves one entry along the row, and a touchpad adds up its
  // smaller deltas until they make a notch's worth.
  property real wheelAccumulator: 0

  function scrollWorkspaces(delta) {
    var wheel = Util.wheelSteps(root.wheelAccumulator, delta)
    root.wheelAccumulator = wheel.remainder
    if (wheel.steps === 0) return

    // Holds at either end rather than wrapping, so a touchpad swipe that runs
    // long stops on the last workspace.
    var ids = root.workspaceIds()
    var index = ids.indexOf(root.activeWorkspaceId())
    if (index === -1) return
    var target = ids[Util.clamp(index - wheel.steps, 0, ids.length - 1)]
    if (target !== ids[index]) root.focusWorkspace(target)
  }

  // The workspace each monitor shows carries its layout: a square tiles, a
  // circle scrolls, and a triangle floats. Clicking it cycles to the next one,
  // as Super + L does.
  property string activeMode: "dwindle"
  property bool modeStale: false

  function modeGlyph(mode) {
    if (mode === "scrolling") return "󰝥"
    if (mode === "floating") return "󰔶"
    return "󱓻"
  }

  // A layout only changes through the layout commands, which ask for a refresh,
  // or a config reload, so each workspace's mode is read once and cached rather
  // than running the script on every workspace switch.
  property var modeCache: ({})
  property int modeGeneration: 0
  property int modeProcWorkspace: -1
  property int modeProcGeneration: 0

  function showActiveMode() {
    var mode = root.modeCache[root.activeWorkspaceId()]
    if (mode !== undefined) root.activeMode = mode
    else readMode()
  }

  function readMode() {
    if (modeProc.running) {
      root.modeStale = true
      return
    }
    var id = root.activeWorkspaceId()
    if (id === -1) return
    root.modeProcWorkspace = id
    root.modeProcGeneration = root.modeGeneration
    modeProc.command = ["omarchy-hyprland-workspace-layout-current", String(id)]
    modeProc.running = true
  }

  function refresh() {
    root.modeCache = ({})
    root.modeGeneration++
    readMode()
  }

  // The layout commands act on the focused workspace, so a click on another
  // monitor's bar focuses that monitor first.
  function cycleLayout() {
    if (!root.bar) return
    var command = "omarchy-hyprland-workspace-layout-toggle"
    if (root.monitor && !root.monitor.focused) {
      command = "hyprctl dispatch " + Util.shellQuote("hl.dsp.focus({ monitor = \"" + root.monitorName + "\" })") + " >/dev/null && " + command
    }
    root.bar.run(command)
  }

  Component.onCompleted: refresh()
  onMonitorChanged: root.showActiveMode()

  Connections {
    target: root.monitor
    function onActiveWorkspaceChanged() { root.showActiveMode() }
  }

  Connections {
    target: Hyprland
    function onRawEvent(event) {
      var name = event ? String(event.name) : ""
      if (name === "configreloaded") root.refresh()
      // Quickshell credits every workspace switch to the focused monitor, as
      // the event doesn't say which one switched, and misses the workspace a
      // monitor falls back to when its own is moved away. Moving workspaces
      // between monitors switches the others, so ask Hyprland for the truth.
      // A workspace that changes number is only learned by asking too. Every
      // bar shares what Hyprland answers, so one asks after a plain switch.
      var switched = name === "workspacev2" && root.monitor !== null && root.monitor.focused
      if (switched || name === "moveworkspacev2" || name === "changeworkspaceid") {
        Hyprland.refreshMonitors()
        Hyprland.refreshWorkspaces()
      }
    }
  }

  Process {
    id: modeProc
    stdout: StdioCollector {
      onStreamFinished: {
        var mode = text.trim()
        // A read that started before a refresh may predate the change.
        if (!mode || root.modeProcGeneration !== root.modeGeneration) return
        root.modeCache[root.modeProcWorkspace] = mode
        if (root.modeProcWorkspace === root.activeWorkspaceId()) root.activeMode = mode
      }
    }
    onExited: {
      if (!root.modeStale) return
      root.modeStale = false
      // Deferred: the process still reports running while it signals exit.
      Qt.callLater(root.showActiveMode)
    }
  }

  // Super + L and the default layout change the mode without a focus change,
  // so they ask every bar to read it again.
  ShellIpc {
    target: "omarchy.workspaces"

    function refresh(): void { root.broadcast("refresh") }
  }

  readonly property real trailingGap: root.vertical ? 0 : Style.spaceReal(1.5)

  implicitWidth: grid.implicitWidth + trailingGap
  implicitHeight: grid.implicitHeight

  GridLayout {
    id: grid
    anchors.fill: parent
    anchors.rightMargin: root.trailingGap
    columns: root.vertical ? 1 : root.workspaceIds().length
    columnSpacing: root.vertical ? 0 : Style.space(1)
    rowSpacing: root.vertical ? Style.space(2) : 0

    Repeater {
      model: root.workspaceIds()

      WidgetButton {
        required property int modelData

        readonly property var workspace: root.workspaceById(modelData)
        readonly property bool occupied: workspace !== null && workspace.toplevels.values.length > 0
        readonly property bool shown: root.monitor !== null && root.monitor.activeWorkspace !== null && root.monitor.activeWorkspace.id === modelData
        readonly property int slot: root.slotOf(modelData)
        readonly property bool firstGuest: root.blockOf(modelData) !== root.block && index > 0 && root.blockOf(root.workspaceIds()[index - 1]) === root.block
        required property int index

        // A gap sets visiting workspaces apart from the monitor's own.
        Layout.leftMargin: firstGuest && !root.vertical ? Style.space(6) : 0
        Layout.topMargin: firstGuest && root.vertical ? Style.space(6) : 0

        bar: root.bar
        // amon: the workspace's own agent state, replacing the number.
        //
        // Keyed by the global id, which is also the workspace's name under
        // Omarchy's defaults — and the daemon places agents by name.
        readonly property string agentState: agents.stateByWorkspace[String(modelData)] || ""
        readonly property string label: slot === root.slots ? "0" : String(slot)
        // The marker for the workspace you are on is upstream's own: the
        // layout glyph, which says where you are and how that workspace
        // tiles in one shape. Configuration may replace it (`focused`), and
        // the state borrows its turns from it — the workspace you are sitting
        // on was the one workspace whose agent the bar would not show you,
        // which is the wrong one to hide: its terminal may well be behind a
        // full-screen window. So a shown workspace holding an agent that
        // wants something takes turns, and the state gets the longer share
        // of them: you already know where you are, the whole desktop says
        // so, while the state is the thing you cannot see without leaving
        // what you are doing. An agent at rest borrows nothing —
        // `agents.showingState` only goes true while the shown workspace
        // holds a state worth the interruption.
        readonly property string marker: agents.focusedGlyph || root.modeGlyph(root.activeMode)
        text: shown && !agents.showingState ? marker
            : agentState === "working" ? agents.spinner                  // braille spinner
            : agentState === "blocked" ? agents.blockedGlyph            // help circle
            : agentState === "done" ? agents.doneGlyph                   // check circle
            : label
        // Two labels upstream's own cannot draw, so a stand-in draws them:
        //
        // An agent that has come to rest asks for nothing, so the workspace
        // keeps its number and underlines it: an agent is here and wants
        // nothing. Bold was tried first and could not be seen — a lone digit
        // on a bar has nothing beside it to be heavier *than* — and not
        // opacity, which upstream already owns for a workspace holding no
        // windows. Upstream's label exposes only family and size, draws
        // plain text since 4.0.4, and the bar's font has no combining low
        // line, so the underline needs a Text of the fork's own.
        //
        // The spinner, because since 4.5 upstream treats any lone character
        // from the arrows-and-symbols blocks as an icon and scales its ink to
        // fill the icon size. Braille lives in those blocks, and frames whose
        // ink boxes differ — which is what makes the dots appear to move —
        // each get blown up to the same size, so the spinner breathes instead
        // of turning. Drawn as text, a frame keeps its cell.
        //
        // `labelVisible` is upstream's own switch for a button that draws its
        // content itself.
        readonly property bool underlined: agentState === "idle" && text === label
        readonly property bool spinning: text === agents.spinner
        readonly property bool standIn: underlined || spinning
        labelVisible: !standIn
        opacity: occupied || shown ? 1 : 0.5
        horizontalMargin: 6
        verticalPadding: 6
        fixedWidth: root.vertical ? root.barSize : Style.space(20)
        fixedHeight: root.barSize
        // amon: a click acts on what the button shows. The marker is
        // upstream's layout glyph, so clicking it cycles the layout as
        // upstream does; the agent's state, in its turn, is a door to that
        // agent.
        onPressed: function() {
          if (shown && !agents.showingState) root.cycleLayout()
          else root.focusWorkspace(modelData)
        }
        onWheelMoved: function(delta) { root.scrollWorkspaces(delta) }
        // The stand-in for upstream's label while `standIn` holds, drawn the
        // way WidgetButton draws its own — the same figure centring and press
        // give, so the digit neither moves nor changes — with the one font
        // property upstream does not expose.
        Text {
          id: standInLabel
          visible: parent.standIn
          anchors.centerIn: parent
          anchors.verticalCenterOffset: !parent.centerFigures || parent.vertical
            ? 0
            : (standInMetrics.descent - standInMetrics.ascent + standInMetrics.tightBoundingRect("0").height) / 2
          textFormat: Text.PlainText
          text: parent.text
          color: parent.active && parent.useActiveColor ? parent.activeColor : parent.foreground
          font.family: parent.fontFamily
          font.pixelSize: parent.fontSize
          font.underline: parent.underlined
          renderType: Text.NativeRendering
          rotation: parent.textRotation
          scale: parent.pressScale
          horizontalAlignment: Text.AlignHCenter
          verticalAlignment: Text.AlignVCenter
          // No blank line above, for the drift test's sake (see focusViaAmon).
          FontMetrics {
            id: standInMetrics
            font: standInLabel.font
          }
        }
      }
    }
  }
}
