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

  readonly property color dim: Qt.darker(foreground, 1.4)

  implicitHeight: column.implicitHeight

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
      foreground: view.foreground
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
      foreground: view.foreground
    }
  }
}
