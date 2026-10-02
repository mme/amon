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

  // The agent and "when" columns are as wide as their widest value, so every
  // row agrees where the location starts and the times read down as one
  // right-aligned column; the location takes the rest and elides into it.
  readonly property int agentWidth: {
    let widest = 0
    for (const row of view.rows) widest = Math.max(widest, view.textWidth(row.agent))
    return Math.min(widest, Style.space(160))
  }
  readonly property int whenWidth: {
    let widest = 0
    for (const row of view.rows) widest = Math.max(widest, view.textWidth(row.when))
    return widest
  }

  // Where a row was, split the way the agent panel splits it: the one segment
  // that identifies it bold, the qualification around it dim. A remote row
  // leads with its host.
  function locationParts(row) {
    const lead = row.host ? row.host + ": " : ""
    if (row.project)
      return { prefix: lead, bold: row.project, suffix: row.subpath ? "/" + row.subpath : "" }
    const path = row.path || row.dir || ""
    const cut = path.lastIndexOf("/")
    if (cut < 0 || path === "~")
      return { prefix: lead, bold: path, suffix: "" }
    return { prefix: lead + path.slice(0, cut + 1), bold: path.slice(cut + 1), suffix: "" }
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
      x: Style.space(10)
      width: view.agentWidth
      anchors.verticalCenter: parent.verticalCenter
      text: row.entry.agent
      color: view.foreground
      font.family: view.fontFamily
      font.pixelSize: Style.font.body
      elide: Text.ElideRight
    }

    // Where: as the agent panel writes it, starting at the column's left edge,
    // the dim lead giving way from the front when space is short.
    Item {
      id: location
      x: Style.space(10) + view.agentWidth + view.columnGap
      width: Math.max(0, row.width - x - view.whenWidth - view.columnGap - Style.space(10))
      height: parent.height

      readonly property var parts: view.locationParts(row.entry)
      readonly property real boldWidth: Math.min(view.textWidth(parts.bold), width)
      readonly property real prefixWidth: parts.prefix === ""
        ? 0
        : Math.min(view.textWidth(parts.prefix), Math.max(0, width - boldWidth))

      Text {
        visible: location.parts.prefix !== ""
        width: location.prefixWidth
        anchors.verticalCenter: parent.verticalCenter
        text: location.parts.prefix
        color: view.dim
        font.family: view.fontFamily
        font.pixelSize: Style.font.body
        elide: Text.ElideLeft
      }

      Text {
        x: location.prefixWidth
        width: location.boldWidth
        anchors.verticalCenter: parent.verticalCenter
        text: location.parts.bold
        color: view.foreground
        font.family: view.fontFamily
        font.pixelSize: Style.font.body
        font.bold: true
      }

      Text {
        visible: location.parts.suffix !== ""
        x: location.prefixWidth + location.boldWidth
        width: Math.max(0, location.width - location.prefixWidth - location.boldWidth)
        anchors.verticalCenter: parent.verticalCenter
        text: location.parts.suffix
        color: view.dim
        font.family: view.fontFamily
        font.pixelSize: Style.font.body
        elide: Text.ElideRight
      }
    }

    // When, right-aligned at the edge like the agent panel's age column.
    Text {
      x: row.width - view.whenWidth - Style.space(10)
      width: view.whenWidth
      anchors.verticalCenter: parent.verticalCenter
      text: row.entry.when
      color: view.dim
      font.family: view.fontFamily
      font.pixelSize: Style.font.body
      horizontalAlignment: Text.AlignRight
    }
  }
}
