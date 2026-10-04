// amon's own — no upstream counterpart.
//
// What the start agent panel shows (ADR-0026): the agents you started, most
// recent first, one row per agent and folder. Picking a row starts that agent
// there again in a new terminal - `amon start` does the starting; this view
// only draws the list `amon start --json` hands it and says which row was
// chosen.
//
// Built from the same parts as the agent panel - Omarchy's PanelHero,
// separator, CursorSurface rows and key catcher - so the two read as one
// family. The rows say where an agent was the way the agent panel says it:
// the repository bold with the path inside it dim, or outside a repository the
// folder bold with the path leading up to it dim, the home written as `~`.
//
// The root is `view` and not `root`, for the reason AgentsView gives.

import QtQuick
import QtQuick.Controls
import Quickshell
import qs.Commons
import qs.Ui

FocusScope {
  id: view

  // The rows from `amon start --json`: { agent, dir, host?, project?,
  // subpath?, path, when }.
  property var rows: []

  property color foreground: Color.menu.text
  property string fontFamily: Style.font.menuFamily
  property int contentSpacing: Style.space(12)

  signal activated(var entry)
  signal closeRequested()

  readonly property color dim: Qt.darker(foreground, 1.4)

  property int selectedIndex: 0

  function resetSelection() {
    view.selectedIndex = 0
  }

  function moveSelection(delta) {
    const count = view.rows.length
    if (count === 0) return
    view.selectedIndex = Math.max(0, Math.min(view.selectedIndex + delta, count - 1))
  }

  function activateSelection() {
    if (view.selectedIndex < 0 || view.selectedIndex >= view.rows.length) return
    view.activated(view.rows[view.selectedIndex])
  }

  onRowsChanged: view.selectedIndex = Math.max(0, Math.min(view.selectedIndex, view.rows.length - 1))

  // Ctrl-N and Ctrl-P, as the agent panel takes them.
  focus: true
  Keys.onPressed: function(event) {
    if (!(event.modifiers & Qt.ControlModifier)) return
    if (event.key === Qt.Key_N) {
      view.moveSelection(1)
      event.accepted = true
    } else if (event.key === Qt.Key_P) {
      view.moveSelection(-1)
      event.accepted = true
    }
  }

  FontMetrics {
    id: metrics
    font.family: view.fontFamily
    font.pixelSize: Style.font.body
  }
  function textWidth(text) {
    return text ? Math.ceil(metrics.advanceWidth(text)) : 0
  }

  readonly property int columnGap: Style.space(10)

  // Four columns, left to right: the agent, the project, the folder and
  // when. Every column but the folder is as wide as its widest value (the
  // project capped), so each starts at the same place
  // on every row; the folder takes the rest and gives way from the front,
  // where the path matters least.
  function widest(field, cap) {
    let widest = 0
    for (const row of view.rows) widest = Math.max(widest, view.textWidth(view.cell(row, field)))
    return cap > 0 ? Math.min(widest, cap) : widest
  }
  readonly property int agentWidth: view.widest("agent", Style.space(160))
  readonly property int projectWidth: view.widest("project", Style.space(200))
  readonly property int whenWidth: view.widest("when", 0)

  // What a cell says. The project is the repository's name, or outside one
  // the folder's own name; the folder is the path with `~` for home, a
  // remote one led by its host.
  function cell(row, field) {
    if (field === "agent") return row.agent || ""
    if (field === "when") return row.when || ""
    if (field === "project") {
      if (row.project) return row.project
      const path = row.path || row.dir || ""
      const cut = path.lastIndexOf("/")
      return cut < 0 ? path : path.slice(cut + 1)
    }
    if (field === "folder") {
      // Blank when it would only repeat the project - an agent started in
      // the home folder is `~` and `~` otherwise. A remote one keeps its
      // host, which the project column does not say.
      const path = row.path || row.dir || ""
      if (!row.host && path === view.cell(row, "project")) return ""
      return (row.host ? row.host + ": " : "") + path
    }
    return ""
  }

  readonly property string summary: view.rows.length > 0 ? view.rows.length + " recent" : "none yet"

  PanelKeyCatcher {
    anchors.fill: parent

    onMoveRequested: function(dx, dy) { if (dy !== 0) view.moveSelection(dy) }
    onActivateRequested: view.activateSelection()
    onCloseRequested: view.closeRequested()

  Column {
    anchors.fill: parent
    spacing: view.contentSpacing

    PanelHero {
      id: hero
      width: parent.width
      title: "start agent"
      meta: view.summary
      foreground: view.foreground
      fontFamily: view.fontFamily

      iconComponent: Component {
        AmonMark {
          iconSize: Style.font.display
          color: view.foreground
        }
      }
    }

    PanelSeparator {
      id: rule
      foreground: view.foreground
    }

    Item {
      id: body
      width: parent.width
      height: Math.max(0, view.height - hero.height - rule.height - view.contentSpacing * 2)

      ListView {
        id: list
        anchors.fill: parent
        visible: view.rows.length > 0
        spacing: Style.space(4)
        clip: true
        boundsBehavior: Flickable.StopAtBounds
        interactive: contentHeight > height

        ScrollBar.vertical: ScrollBar { policy: ScrollBar.AsNeeded }

        model: view.rows
        currentIndex: view.selectedIndex
        onCurrentIndexChanged: if (currentIndex >= 0) positionViewAtIndex(currentIndex, ListView.Contain)

        delegate: StartRow {
          required property var modelData
          required property int index
          width: ListView.view.width
          entry: modelData
          rowIndex: index
        }
      }

      // Nothing started yet. The agent panel's placeholder, in its words.
      Column {
        visible: view.rows.length === 0
        anchors.centerIn: parent
        width: Math.min(parent.width - Style.space(48), Style.space(380))
        spacing: Style.space(10)

        Text {
          width: parent.width
          horizontalAlignment: Text.AlignHCenter
          text: "No agents yet"
          color: view.foreground
          font.family: view.fontFamily
          font.pixelSize: Style.font.subtitle
          font.bold: true
        }

        Text {
          width: parent.width
          horizontalAlignment: Text.AlignHCenter
          wrapMode: Text.WordWrap
          text: "Start one in a terminal and it shows up here."
          color: view.dim
          font.family: view.fontFamily
          font.pixelSize: Style.font.body
          lineHeight: 1.35
        }
      }
    }
  }
  }

  component StartRow: CursorSurface {
    id: row

    required property var entry
    required property int rowIndex

    hasCursor: view.selectedIndex === rowIndex
    foreground: view.foreground
    implicitHeight: Math.round(Style.font.body * 2.4)

    MouseArea {
      anchors.fill: parent
      hoverEnabled: true
      onEntered: view.selectedIndex = row.rowIndex
      onClicked: view.activated(row.entry)
    }

    // The agent, in plain ink: what will run.
    Text {
      id: agentCell
      x: Style.space(10)
      width: view.agentWidth
      anchors.verticalCenter: parent.verticalCenter
      text: view.cell(row.entry, "agent")
      color: view.foreground
      font.family: view.fontFamily
      font.pixelSize: Style.font.body
      elide: Text.ElideRight
    }

    // The project, bold: what identifies the row, as on the agent panel.
    Text {
      id: projectCell
      x: agentCell.x + view.agentWidth + view.columnGap
      width: view.projectWidth
      anchors.verticalCenter: parent.verticalCenter
      text: view.cell(row.entry, "project")
      color: view.foreground
      font.family: view.fontFamily
      font.pixelSize: Style.font.body
      font.bold: true
      elide: Text.ElideRight
    }

    // The folder, dim. When it is longer than its column, every row shows its
    // end - the folder's own name - and the row under the cursor reads the
    // whole of it the way a music player shows a long title: from the start,
    // gliding slowly to the end, resting there a good while, then back to the
    // start quickly, and again. Only the cursor's row moves, so the list never
    // has two lines moving at once.
    Item {
      id: folderCell
      x: projectCell.x + view.projectWidth + view.columnGap
      width: Math.max(0, whenCell.x - x - view.columnGap)
      height: parent.height
      clip: true

      readonly property string full: view.cell(row.entry, "folder")
      readonly property real overflow: Math.max(0, view.textWidth(full) - width)
      readonly property bool gliding: overflow > 0 && row.hasCursor
      // A calm reading pace, and never so short a glide that it jumps.
      readonly property int glideMs: Math.max(1200, Math.round(overflow / 28 * 1000))
      // How far along the path the view is: 0 shows its start, 1 its end.
      // A fraction rather than pixels, so the position follows the column's
      // width as it settles instead of keeping one measured before it had.
      property real along: 0

      onGlidingChanged: along = 0

      Text {
        x: folderCell.gliding ? -folderCell.overflow * folderCell.along : 0
        width: folderCell.gliding ? implicitWidth : folderCell.width
        anchors.verticalCenter: parent.verticalCenter
        text: folderCell.full
        color: view.dim
        font.family: view.fontFamily
        font.pixelSize: Style.font.body
        elide: folderCell.gliding ? Text.ElideNone : Text.ElideLeft
      }

      SequentialAnimation {
        running: folderCell.gliding && view.visible
        loops: Animation.Infinite
        // A moment on the start, a slow read to the end, a long rest there,
        // and a quick return.
        PauseAnimation { duration: 1500 }
        NumberAnimation {
          target: folderCell; property: "along"; to: 1
          duration: folderCell.glideMs; easing.type: Easing.InOutSine
        }
        PauseAnimation { duration: 4000 }
        NumberAnimation {
          target: folderCell; property: "along"; to: 0
          duration: 450; easing.type: Easing.InOutCubic
        }
        onRunningChanged: if (!running) folderCell.along = 0
      }
    }

    // When, right-aligned at the edge like the agent panel's age column.
    Text {
      id: whenCell
      x: row.width - view.whenWidth - Style.space(10)
      width: view.whenWidth
      anchors.verticalCenter: parent.verticalCenter
      text: view.cell(row.entry, "when")
      color: view.dim
      font.family: view.fontFamily
      font.pixelSize: Style.font.body
      horizontalAlignment: Text.AlignRight
    }
  }
}
