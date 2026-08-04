// Forked from Omarchy (ADR-0008). Do not edit the upstream parts by hand.
//
//   upstream: /usr/share/omarchy/shell/plugins/bar/widgets/Workspaces.qml
//   taken:    2026-08-02, Omarchy Quattro (Hyprland 0.56.1, quickshell-git)
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
// the three changes, each marked `amon:` below —
//
//   1. `moduleName` names this plugin rather than Omarchy's
//   2. the `AgentStates` object
//   3. the `agentState` property and the `text` expression that reads it
//
// Anything else differing from upstream is drift, and the tests in fork.rs and
// desktop.rs exist to notice it.

import QtQuick
import QtQuick.Layouts
import Quickshell.Hyprland
import qs.Commons
import qs.Ui

BarWidget {
  id: root
  moduleName: "sh.amon.workspaces"   // amon: upstream says omarchy.workspaces

  // amon: the one piece of state upstream does not have.
  AgentStates { id: agents }

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
        required property int modelData

        readonly property var workspace: root.workspaceById(modelData)
        readonly property bool occupied: workspace !== null && workspace.toplevels.values.length > 0
        readonly property bool focused: Hyprland.focusedWorkspace !== null && Hyprland.focusedWorkspace.id === modelData

        bar: root.bar
        // amon: the workspace's own agent state, replacing the number.
        //
        // Built with String.fromCodePoint rather than "\uXXXX": QML's escape
        // takes exactly four hex digits, and the Material icons live above
        // U+FFFF, so "\uF051F" would silently parse as U+F051 followed by "F".
        readonly property string agentState: agents.stateByWorkspace[String(modelData)] || ""
        readonly property string label: modelData === 10 ? "0" : String(modelData)
        // Focus wins: which workspace you are on is something only this widget
        // says, while an agent's state is also one `amon status` away.
        text: focused ? "󱓻"
            : agentState === "working" ? agents.spinner                  // braille spinner
            : agentState === "blocked" ? String.fromCodePoint(0xF02D7)   // help circle
            : agentState === "done" ? String.fromCodePoint(0xF05E0)      // check circle
            // An agent that has come to rest asks for nothing, so the workspace
            // goes back to its number — in bold, which says an agent is here
            // and wants nothing. Weight is free to mean that: upstream says
            // occupancy with opacity, dimming a workspace that holds no
            // windows, and never varies weight itself. Markup rather than a
            // font property because the label exposes only family and size.
            : agentState === "idle" ? "<b>" + label + "</b>"
            : label
        opacity: occupied || focused ? 1 : 0.5
        horizontalMargin: 6
        verticalPadding: 6
        fixedWidth: root.vertical ? root.barSize : Style.space(20)
        fixedHeight: root.barSize
        onPressed: function() { root.focusWorkspace(modelData) }
      }
    }
  }
}
