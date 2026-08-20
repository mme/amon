// amon's own — no upstream counterpart.
//
// What the pane shows, independent of what is showing it. The same view appears
// inside the Super+A modal and inside the popped-out window, so the two cannot
// drift into being two different views of one thing.
//
// It knows nothing about layer shells, windows or dismissal. It is handed a
// model and draws it, and the one thing it asks of its host is to pop out —
// which it asks for rather than does, because only the host knows whether it is
// the modal (which can) or the window (which already has).
//
// The root is `view` and not `root`: inside a PanelHero's `trailingControl`,
// `root` resolves to the hero itself, so anything reaching back here through
// that name would quietly read the wrong object. The Tailscale panel hits the
// same edge and works around it the same way.

import QtQuick
import QtQuick.Controls
import Quickshell
import qs.Commons
import qs.Ui

Item {
  id: view

  // The shared AgentStates instance. Handed in rather than created here, so the
  // modal and the window read one socket between them.
  required property var agents

  property color foreground: Color.menu.text
  property color background: Color.menu.background
  property string fontFamily: Style.font.menuFamily
  property int contentSpacing: Style.space(12)

  // True in the popped-out window, which has nowhere left to pop out to.
  property bool poppedOut: false

  // Whether that window is floating. Tiled, it is an ordinary window the
  // desktop moves for you, and the hint below would be telling you to do by
  // hand what the tiler is already doing.
  property bool floating: true

  signal popOutRequested()

  // A row was chosen. The host decides what that means — the modal goes to the
  // agent and closes, the window has nothing to close.
  signal activated(var entry)

  // Which row the cursor is on. There is always one while there are rows at
  // all: a list you arrow into from nowhere makes you press a key to find out
  // where you are.
  property int selectedIndex: 0

  function clampSelection() {
    const count = view.agents.rows.length
    view.selectedIndex = count === 0 ? 0 : Math.max(0, Math.min(view.selectedIndex, count - 1))
  }

  function moveSelection(delta) {
    const count = view.agents.rows.length
    if (count === 0) return
    view.selectedIndex = Math.max(0, Math.min(view.selectedIndex + delta, count - 1))
  }

  function activateSelection() {
    const rows = view.agents.rows
    if (view.selectedIndex < 0 || view.selectedIndex >= rows.length) return
    view.activated(rows[view.selectedIndex])
  }

  // Agents come and go while the pane is open. Without this the cursor would
  // keep an index that no longer exists and the list would show no selection
  // at all.
  Connections {
    target: view.agents
    function onRowsChanged() { view.clampSelection() }
  }

  // Paths are shown the way a person writes them.
  readonly property string home: Quickshell.env("HOME") || ""
  function shortPath(path) {
    if (view.home !== "" && path.indexOf(view.home) === 0)
      return "~" + path.slice(view.home.length)
    return path
  }

  readonly property color dim: Qt.darker(foreground, 1.4)

  // What each state is called in the header. One word per state and no more,
  // because this is a status line read at a glance and not a legend.
  readonly property var labels: ({
    blocked: "needs input",
    done: "done",
    working: "running",
    idle: "idle"
  })

  // "1 NEEDS INPUT · 2 DONE · 1 RUNNING · 3 IDLE", dropping any part that is
  // zero so the line only ever states what is true. PanelHero uppercases it and
  // spaces the letters out; the separator dot is Omarchy's own.
  //
  // Ordered by walking the model's ranking rather than by writing the four out
  // again here. That ranking is `AgentEntry::attention`, the same order
  // `amon status` prints and `amon focus` visits, so the most urgent figure is
  // leftmost for the same reason it is first everywhere else — and a state
  // added to amon cannot end up ordered one way here and another way there.
  readonly property string summary: {
    const counts = view.agents.counts
    const parts = []
    for (const state of view.agents.order) {
      const count = counts[state] || 0
      if (count > 0) parts.push(count + " " + view.labels[state])
    }
    return parts.length > 0 ? parts.join(" · ") : "no agents"
  }

  // Offered while the pane is a modal: the way out to a window of its own.
  Component {
    id: popOutButton

    // `Button` and not `PanelActionButton`: the latter is a square that holds
    // one glyph, and this needs a word beside it. Omarchy's own note on Button
    // is "one component for every clickable thing in the kit", and it takes an
    // icon and a label together — so the whole rectangle is the target, not
    // just the glyph in it.
    Button {
      // Material Design's picture-in-picture, in the family every other icon in
      // this shell comes from. It promises a small pane that stays put over
      // everything else, and the window rule amon installs makes that true: the
      // window opens floating and pinned, so it is on every workspace until you
      // send it back with Super+O.
      iconText: "󰹙"
      // The bracket marks the key that does this, the way `[Super+Drag to move]`
      // does in the popped-out window — brackets mean "keyboard" throughout this
      // pane. Omarchy has no such convention: its own panels carry bare-letter
      // shortcuts that appear nowhere on screen, which is why nobody knows the
      // Tailscale panel copies an IP on `c`. Showing the key is worth departing
      // from that.
      text: "[F]loat"
      tooltipText: "Open in a floating window that stays on screen"
      // Muted, at the same weight as `[Super+Drag to move]` — the two are the
      // same kind of thing in the same corner: an aside about the window, not
      // anything about your agents. Button paints both its glyph and its label
      // from `foreground`, so this dims the pair together.
      foreground: view.dim
      fontFamily: view.fontFamily
      onClicked: view.popOutRequested()
    }
  }

  // And what stands in its place once that has happened. It sits exactly where
  // the button was, so the header keeps its shape and the corner that offered
  // the mode is the corner that says what to do with it.
  //
  // It says how to move the window, because nothing else does. A pinned
  // floating window has no titlebar to grab — Omarchy draws none — so the only
  // way to move it is the compositor's own `SUPER + mouse:272`, bound to
  // `hl.dsp.window.drag()` in Omarchy's tiling bindings under the name "Move
  // window". Someone who has not met that binding has a pane they cannot get
  // out of the way.
  //
  // Quiet, and bracketed. This is an aside about how to work the window, not
  // anything the pane is telling you about your agents, and it should lose
  // every contest for attention with the line above it.
  Component {
    id: moveHint

    Text {
      text: "[Super+Drag to move]"
      color: view.dim
      font.family: view.fontFamily
      font.pixelSize: Style.font.body
    }
  }

  Column {
    id: column
    anchors.fill: parent
    spacing: view.contentSpacing

    // Omarchy's own panel header — the same component the Tailscale panel uses,
    // so the mark, the name and the status line sit exactly where they sit
    // there, at the same sizes, without repeating its geometry.
    PanelHero {
      id: hero
      width: parent.width
      title: "amon"
      meta: view.summary
      foreground: view.foreground
      fontFamily: view.fontFamily

      iconComponent: Component {
        AmonMark {
          iconSize: Style.font.display
          color: view.foreground
        }
      }

      // The trailing edge is where the Tailscale panel puts its power switch,
      // so it is where this desktop expects a header's control to be.
      trailingControl: Component {
        Loader {
          // Nothing at all once the window is tiled: the desktop is placing
          // it, so there is neither a mode to report nor a way out to offer.
          sourceComponent: view.poppedOut
            ? (view.floating ? moveHint : null)
            : popOutButton
        }
      }
    }

    PanelSeparator {
      id: rule
      foreground: view.foreground
    }

    // One flat list with the workspace headings folded into it, the way the
    // network panel separates known networks from the others. A ListView rather
    // than a Repeater in a Column, for the same reason it uses one:
    // `positionViewAtIndex` is what keeps the cursor on screen as it walks past
    // the bottom of the visible window.
    ListView {
      id: list

      width: parent.width
      // Whatever the header leaves. Measured from the pieces above rather
      // than from this item's own content, which would be a loop: the column
      // would size to the list and the list to the column.
      height: Math.max(0, view.height - hero.height - rule.height - view.contentSpacing * 2)
      spacing: Style.space(4)
      clip: true
      boundsBehavior: Flickable.StopAtBounds
      interactive: contentHeight > height

      ScrollBar.vertical: ScrollBar { policy: ScrollBar.AsNeeded }

      model: view.agents.rows
      currentIndex: view.selectedIndex
      onCurrentIndexChanged: if (currentIndex >= 0) positionViewAtIndex(currentIndex, ListView.Contain)

      // The wrapper takes the delegate context's properties and hands them down
      // explicitly, because a nested `component` declaration does not inherit
      // them. Same shape as the network panel's delegate, for the same reason.
      delegate: Item {
        required property var modelData
        required property int index

        readonly property string heading: view.agents.sectionTitle(index)

        width: ListView.view.width
        height: rowColumn.implicitHeight

        Column {
          id: rowColumn
          width: parent.width
          spacing: Style.space(4)

          PanelSectionHeader {
            visible: heading !== ""
            text: heading
            foreground: view.foreground
            fontFamily: view.fontFamily
            height: visible ? implicitHeight : 0
          }

          AgentRow {
            width: parent.width
            entry: modelData
            index: parent.parent.index
          }
        }
      }
    }
  }

  // One agent. The columns are fixed widths so that they line up down the list;
  // only the path takes what is left, and it elides from the left because the
  // end of a path is the part that identifies it.
  component AgentRow: CursorSurface {
    id: row

    required property var entry
    required property int index

    readonly property bool isSelected: view.selectedIndex === index

    hasCursor: isSelected
    foreground: view.foreground
    implicitHeight: Math.round(Style.font.body * 2.4)

    MouseArea {
      anchors.fill: parent
      hoverEnabled: true
      // Hover moves the cursor rather than drawing a second highlight, so there
      // is only ever one row that looks chosen.
      onEntered: view.selectedIndex = row.index
      onClicked: view.activated(row.entry)
    }

    // The state, as the glyph amon already uses for it everywhere else — the
    // same characters the bar draws, configuration included, so changing a
    // glyph in the config changes it in both places. A working agent turns the
    // same spinner in step with the bar's.
    //
    // Idle has no glyph on purpose: it is the absence of anything happening,
    // and the column keeps its width so the names stay aligned.
    Text {
      id: stateGlyph
      anchors.left: parent.left
      anchors.leftMargin: Style.space(10)
      anchors.verticalCenter: parent.verticalCenter
      width: Style.space(18)
      text: {
        if (row.entry.state === "blocked") return view.agents.blockedGlyph
        if (row.entry.state === "done") return view.agents.doneGlyph
        if (row.entry.state === "working") return view.agents.spinner
        return ""
      }
      color: row.entry.state === "blocked" ? Color.urgent : view.foreground
      font.family: view.fontFamily
      font.pixelSize: Style.font.body
    }

    Text {
      id: agentName
      anchors.left: stateGlyph.right
      anchors.leftMargin: Style.space(8)
      anchors.verticalCenter: parent.verticalCenter
      width: Style.space(78)
      text: row.entry.agent
      color: view.foreground
      font.family: view.fontFamily
      font.pixelSize: Style.font.body
      font.bold: true
      elide: Text.ElideRight
    }

    Text {
      id: ageText
      anchors.right: parent.right
      anchors.rightMargin: Style.space(10)
      anchors.verticalCenter: parent.verticalCenter
      width: Style.space(34)
      horizontalAlignment: Text.AlignRight
      text: view.agents.age(row.entry.stateSince, view.now)
      color: view.dim
      font.family: view.fontFamily
      font.pixelSize: Style.font.body
    }

    Text {
      id: stateText
      anchors.right: ageText.left
      anchors.rightMargin: Style.space(10)
      anchors.verticalCenter: parent.verticalCenter
      width: Style.space(62)
      text: view.labels[row.entry.state] || row.entry.state
      color: row.entry.state === "blocked" ? Color.urgent : view.dim
      font.family: view.fontFamily
      font.pixelSize: Style.font.body
      elide: Text.ElideRight
    }

    Text {
      anchors.left: agentName.right
      anchors.leftMargin: Style.space(8)
      anchors.right: stateText.left
      anchors.rightMargin: Style.space(10)
      anchors.verticalCenter: parent.verticalCenter
      text: view.shortPath(row.entry.cwd)
      color: view.dim
      font.family: view.fontFamily
      font.pixelSize: Style.font.body
      elide: Text.ElideLeft
    }
  }

  // The clock the ages are measured against. It is a property rather than a
  // call to `Date.now()` inside the row, because a binding only re-runs when
  // something it *reads* changes — reading the clock directly would freeze each
  // age at whatever it was when the row was built. The daemon has no reason to
  // resend an agent just because a minute passed, so nothing else would move.
  property double now: Date.now()

  Timer {
    running: true
    repeat: true
    interval: 1000
    onTriggered: view.now = Date.now()
  }
}
