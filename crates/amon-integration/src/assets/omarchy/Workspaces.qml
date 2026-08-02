// Forked from Omarchy's plugins/bar/widgets/Workspaces.qml (ADR-0008).
//
// Omarchy's bar has no way for one plugin to decorate another's widget, and a
// dot above the workspace numbers can only be drawn by whatever draws the
// numbers. So this is upstream's widget with one layer added and nothing else
// changed — same rendering, same click behaviour — which is what keeps a
// re-sync a re-application rather than a merge.
//
// The amon additions are exactly: the `dots` object below, and the `Rectangle`
// inside WidgetButton. Everything else is upstream and must be re-derived from
// it rather than edited here.

import QtQuick
import QtQuick.Layouts
import Quickshell.Hyprland
import qs.Commons
import qs.Ui

BarWidget {
  id: root
  moduleName: "sh.amon.workspaces"

  // amon: the one piece of state upstream does not have.
  AgentDots { id: dots }

  function workspaceById(id) {
    var values = Hyprland.workspaces.values
    for (var i = 0; i < values.length; i++) {
      if (values[i].id === id) return values[i]
    }

    return null
  }

  function workspaceIds() {
    var ids = [1, 2, 3, 4, 5]
    var values = Hyprland.workspaces.values

    for (var i = 0; i < values.length; i++) {
      var id = values[i].id
      if (id > 0 && id <= 10 && ids.indexOf(id) === -1) ids.push(id)
    }

    ids.sort(function(left, right) { return left - right })
    return ids
  }

  function focusWorkspace(id) {
    if (!root.bar) return
    root.bar.run("hyprctl dispatch " + Util.shellQuote("hl.dsp.focus({ workspace = \"" + id + "\" })"))
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
        id: button
        required property int modelData

        readonly property var workspace: root.workspaceById(modelData)
        readonly property bool occupied: workspace !== null && workspace.toplevels.values.length > 0
        readonly property bool focused: Hyprland.focusedWorkspace !== null && Hyprland.focusedWorkspace.id === modelData

        bar: root.bar
        text: focused ? "󱓻" : (modelData === 10 ? "0" : String(modelData))
        opacity: occupied || focused ? 1 : 0.5
        horizontalMargin: 6
        verticalPadding: 6
        fixedWidth: root.vertical ? root.barSize : Style.space(20)
        fixedHeight: root.barSize
        onPressed: function() { root.focusWorkspace(modelData) }

        // amon: agent state for this workspace. Sits inside the button's own
        // top padding so the bar keeps its height, and is absent entirely when
        // the workspace has no agents.
        Rectangle {
          readonly property string agentState: dots.stateByWorkspace[String(button.modelData)] || ""

          visible: agentState !== ""
          width: 5
          height: 5
          radius: width / 2
          anchors.horizontalCenter: parent.horizontalCenter
          anchors.top: parent.top
          anchors.topMargin: 2

          // Hollow for an idle agent already seen; filled for anything that is
          // new, working, or waiting.
          color: agentState === "idle" ? "transparent"
               : agentState === "working" ? "#304FFE"
               : agentState === "done" ? "#00C853"
               : agentState === "blocked" ? "#FF6D00"
               : "transparent"
          border.width: agentState === "idle" ? 1 : 0
          border.color: "#9E9E9E"
          // The dot reports the agent, not the workspace's own dimming.
          opacity: button.opacity > 0 ? 1 / button.opacity : 1
        }
      }
    }
  }
}
