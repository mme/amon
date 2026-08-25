// amon's own — no upstream counterpart.
//
// The pane Super+A opens: a centred card over a dimmed desktop, dismissed by
// the same key, by Escape, or by clicking away from it.
//
// Deliberately empty. What goes inside is the next piece of work, and getting
// the surface right first means that work is about agents rather than about
// layer shells and focus.
//
// The shape is Omarchy's own overlays (emojis, the image picker): one
// full-screen PanelWindow on the overlay layer, a scrim, a card centred in it,
// and `shell.hide(id)` on the way out so the shell's idea of what is open stays
// in step with ours. Following it exactly is what makes this look like part of
// the desktop rather than a window amon drew.

import Quickshell
import Quickshell.Hyprland
import Quickshell.Io
import Quickshell.Wayland
import QtQuick
import qs.Commons
import qs.Ui

Item {
  id: root

  // Set by the shell when it loads a plugin. `manifest.id` is what `hide`
  // wants, so the id lives in the manifest and not in a second place here.
  property var shell: null
  property var manifest: null

  property bool opened: false

  // The [menu] surface tokens, so a theme that styles Omarchy's own menus
  // styles this too. Nothing here invents a colour.
  readonly property color surface: Color.menu.background
  readonly property color scrim: Color.menu.scrim
  readonly property color foreground: Color.menu.text
  readonly property string fontFamily: Style.font.menuFamily
  readonly property var borderSpec: Border.surfaceSpec("menu", "border", Color.menu.border,
                                                       Math.max(1, Style.space(2)))

  // The card's inner margin, and the gap between what sits inside it. Both are
  // the Tailscale panel's, so a header built from its components is inset the
  // way its own header is: `popupPadding` is what KeyboardPanel insets that
  // card by, and `space(12)` is the gap it leaves between the hero and the rule.
  //
  // The clipboard's `panelPadding` (18) was here first, on the reasoning that
  // this card is centred and large like the clipboard's rather than a small
  // popup hanging off a bar icon. It is the more defensible number in the
  // abstract and it is not the one asked for — the point of this header is to
  // be the same header, and 4px of extra air is exactly the sort of difference
  // that reads as "nearly".
  readonly property int contentMargin: Style.spacing.popupPadding
  readonly property int contentSpacing: Style.space(12)

  // What the header counts. The same model the bar widget uses, so the two
  // cannot disagree about how many agents are waiting on you.
  AgentStates {
    id: agents

    // Which workspace you are on, learned the way the bar widget's instance
    // learns it. The opening pick skips where you are and rotates from it;
    // left unset this stays "", every comparison against it is quietly false,
    // and the cursor just always starts at the top with nothing to say why.
    focusedWorkspace: Hyprland.focusedWorkspace ? String(Hyprland.focusedWorkspace.id) : ""

    // And which window, so the pick can tell two agents on one workspace
    // apart. Normalised to the daemon's coinage — lowercase, no 0x; the
    // wrapper strips both from its tokens, and a comparison that fails on
    // case would not fail loudly, it would just never skip anything.
    focusedWindow: {
      const top = Hyprland.activeToplevel
      const address = top && top.address ? String(top.address) : ""
      return address.toLowerCase().replace(/^0x/, "")
    }
  }

  // How wide the pane wants to be, before any screen is taken into account.
  // Derived from what a row has to hold: agent, path, branch, state and age
  // come to about 69 monospace characters, and the menu font advances exactly
  // 0.6em, so at body size that is roughly 500px of text plus the column gaps,
  // the status icon and the card's own padding.
  //
  // The window reads this figure directly and the modal clamps it to the
  // screen. They must not share the clamped one: `panel` does not exist while
  // the modal is closed, so a window sized from it opens at its minimum.
  readonly property int paneWidth: Style.space(620)
  readonly property int paneHeight: Style.space(600)

  // Sized the way Omarchy sizes its own centred panes — a fixed size, capped by
  // the screen less the gaps.
  //
  // A proportion of the screen was tried and dropped. It kept the pane the same
  // *relative* size everywhere, which sounds right and is not what the desktop
  // does: Omarchy's panes are a fixed size that stops growing, so a pane that
  // scaled with the monitor would be the one surface behaving differently.
  readonly property int cardWidth: Math.min(root.paneWidth,
                                            panel.width - Style.gapsOut * 2)
  readonly property int cardHeight: Math.min(root.paneHeight,
                                             panel.height - Style.gapsOut * 2)

  // The popped-out window's title. Hyprland's own class for every Quickshell
  // window is `org.quickshell`, shared with the rest of the shell, so a window
  // rule can only single this one out by title — which makes this string part
  // of amon's interface with the compositor, not a label. A test holds it to
  // the rule in ~/.config/hypr/amon.lua, because a rule that matches nothing
  // is not an error: the window simply opens tiled and unpinned, and the icon
  // that promised otherwise is the only thing that says so.
  readonly property string windowTitle: "amon — agents"

  property bool windowOpen: false

  // Whether that window is floating right now, straight from Hyprland rather
  // than remembered from when it opened. The window rule floats it, but Super+O
  // puts it back into the tiling, and the pane has to notice: the move hint it
  // draws is only true while nothing else is placing the window.
  //
  // Matched by title for the same reason the window rule is — every Quickshell
  // window is class `org.quickshell`, so the title is all there is to go on.
  readonly property bool windowFloating: {
    const list = Hyprland.toplevels ? Hyprland.toplevels.values : []
    for (let i = 0; i < list.length; i++) {
      if (list[i].title !== root.windowTitle) continue
      const ipc = list[i].lastIpcObject
      return !!(ipc && ipc.floating)
    }
    return false
  }

  // Hyprland does not push a window's floating state; it announces that one
  // changed and expects to be asked. Without this the hint would be right when
  // the window opened and wrong ever after.
  Connections {
    target: Hyprland

    function onRawEvent(event) {
      if (event.name === "changefloatingmode" || event.name === "openwindow")
        Hyprland.refreshToplevels()
    }
  }

  // Leaving the modal for a real window. The modal closes on the way out, so
  // the list is in one place rather than two, and the shell is told — its idea
  // of what is open has to stay in step or the next Super+A toggles nothing.
  //
  // The plugin is `keepLoaded`, which is what makes this safe: shell.qml keeps
  // the Loader active regardless of whether the plugin is "open", so telling
  // the shell we have closed does not take the window down with the modal.
  function popOut() {
    root.windowOpen = true
    root.dismiss()
  }

  // What Super+A reaches. The shell drives this directly: `shell.toggle` reads
  // our `opened` and then calls `open()` or `close()` — the plugin's own
  // `toggle` is never invoked, so the decision has to live here.
  function open() {
    // Already out in a window: send the key there rather than opening a second
    // copy of the same list. Two views of one thing, one of them pinned over
    // the other, is not a pane that answered — it is a pane to tidy up.
    if (root.windowOpen) {
      focusWindow.running = true
      return
    }
    root.opened = true
    // Re-picked on every open, because every open is a fresh "what needs me
    // now" — the answer from ten minutes ago is the one thing this must not
    // carry.
    modalView.resetSelection()
    // Focus after the window exists, or the key catcher has nothing to take
    // focus on and Escape does nothing until something else is clicked.
    Qt.callLater(function(){ modalView.forceActiveFocus() })
  }

  function close() {
    root.opened = false
  }

  // Closing on our own account: the shell still believes this is open, so it
  // has to be told, or the next Super+A toggles it shut instead of open.
  function dismiss() {
    root.opened = false
    if (root.shell && typeof root.shell.hide === "function")
      root.shell.hide((root.manifest && root.manifest.id) || "sh.amon.panel")
  }

  // Super+W's question, asked through the shell's `call` IPC by the bind amon
  // installs (#39): stock Hyprland closes the focused *toplevel*, which with
  // the modal open is the window underneath it. "dismissed" means the pane
  // took the keypress; any other answer — including the shell's own "unknown"
  // when the plugin is not loaded — lets the bind fall through to the close
  // the key always meant. The popped-out window is a real toplevel, `opened`
  // is false while it stands, so it keeps the stock close.
  function dismissIfOpen() {
    if (!root.opened) return "closed"
    root.dismiss()
    return "dismissed"
  }

  // Going to the agent a row names. The daemon hands out the compositor's own
  // token for the window; it is passed back untouched rather than parsed, which
  // is the whole contract on that field. Absent off a supported compositor, so
  // a row without one simply does nothing rather than dispatching nonsense.
  //
  // The modal closes on the way — you asked to be somewhere else.
  function goTo(entry) {
    if (!entry || !entry.window) return
    goToAgent.command = ["hyprctl", "dispatch",
                         "hl.dsp.focus({ window = \"address:0x" + entry.window + "\" })"]
    goToAgent.running = true
    if (root.opened) root.dismiss()
  }

  Process {
    id: goToAgent
    running: false
    command: []
  }

  // Focus by title, because that is all Hyprland can match on: every Quickshell
  // window shares the class `org.quickshell`. Anchored, so it means this window
  // and not one whose title happens to begin the same way.
  //
  // Through hyprctl rather than by asking the window to raise itself: Wayland
  // does not let a client take focus on its own, and the compositor is the only
  // thing that can hand it over.
  Process {
    id: focusWindow
    running: false
    command: ["hyprctl", "dispatch",
              "hl.dsp.focus({ window = \"title:^" + root.windowTitle + "$\" })"]
  }

  PanelWindow {
    id: panel

    visible: root.opened
    // Anchored to all four edges: the window is the whole screen, and the card
    // is centred inside it. That is what gives the scrim something to cover and
    // the click-away somewhere to land.
    anchors { top: true; bottom: true; left: true; right: true }
    color: "transparent"

    WlrLayershell.namespace: "amon-panel"
    WlrLayershell.layer: WlrLayer.Overlay
    // Exclusive, so the pane answers the keyboard while it is up — Escape has
    // to work without clicking first.
    WlrLayershell.keyboardFocus: WlrKeyboardFocus.Exclusive
    // Respects other surfaces' exclusive zones rather than ignoring them, which
    // means this surface is the screen minus the bar, and the card centred in it
    // lands where a centred *window* lands.
    //
    // That is the whole reason for it. Hyprland centres and positions windows
    // within the usable area always — `center`, `move` with percentages, every
    // form of it — so a modal centred on the full screen sits half the bar's
    // height above the window it pops out into, and the pane jumps on the way
    // out. Matching from this side costs one line and stays correct whatever
    // height the bar is, where correcting the window would mean writing half
    // the bar's height into a Hyprland rule.
    //
    // The visible cost is that the scrim stops at the bar instead of dimming
    // it. Omarchy's own modals dim it; this one leaves it lit.
    exclusionMode: ExclusionMode.Normal

    Rectangle {
      anchors.fill: parent
      color: root.scrim
    }

    // Anywhere that is not the card. Below the card in stacking order, so the
    // card's own area never reaches it.
    MouseArea {
      anchors.fill: parent
      onClicked: root.dismiss()
    }

    BorderSurface {
      id: card

      width: root.cardWidth
      height: root.cardHeight
      anchors.centerIn: parent
      radius: Style.cornerRadius
      color: root.surface
      borderSpec: root.borderSpec
      padding: root.contentMargin

      // Swallows clicks so that pressing inside the card is not "outside".
      // Empty on purpose: with nothing to click yet, this is the only thing
      // standing between a stray click and the pane vanishing.
      MouseArea {
        anchors.fill: parent
        onClicked: {}
      }

      AgentsView {
        id: modalView

        anchors.fill: parent
        anchors.topMargin: card.contentTopInset
        anchors.rightMargin: card.contentRightInset
        anchors.bottomMargin: card.contentBottomInset
        anchors.leftMargin: card.contentLeftInset

        agents: agents
        foreground: root.foreground
        fontFamily: root.fontFamily
        contentSpacing: root.contentSpacing
        focus: true
        onPopOutRequested: root.popOut()
        onActivated: function(entry) { root.goTo(entry) }
        onCloseRequested: root.dismiss()
      }
    }
  }

  // The same view, in a window the compositor manages like any other. Not a
  // second app: this belongs to the omarchy-shell process, so it goes when the
  // shell restarts — including when `amon setup` updates the plugin.
  FloatingWindow {
    id: popped

    visible: root.windowOpen
    title: root.windowTitle
    color: root.surface

    // The size the window asks for. Where it lands and whether it floats is
    // the window rule's business, not QML's — the compositor is better placed
    // to decide that. Super+O changes what this window is doing rather than
    // what the rule says: the rule still applies to the next one that maps
    // under this title.
    // The same figures the modal is built from, so popping out moves the pane
    // rather than resizing it. Only the screen clamp differs, and only because
    // a window the compositor places cannot be clamped to a surface that is not
    // on screen at the time.
    implicitWidth: root.paneWidth
    implicitHeight: root.paneHeight
    minimumSize: Qt.size(Style.space(420), Style.space(220))

    // A window closed by its titlebar, by Super+W, or by anything else the
    // desktop offers has to leave `windowOpen` false, or the pane believes it
    // is still out there and the icon never comes back.
    //
    // And on the way in, the size is set rather than left to `implicitWidth`.
    // This window outlives being closed — that is what `keepLoaded` buys, and
    // what lets the modal shut without taking it down — so it carries whatever
    // geometry it last had. Tile it with Super+T, or drag its edge, and the
    // implicit size stops applying; reopening would then hand back the tile's
    // dimensions rather than the pane's, and popping out would resize the pane
    // again. Assigned, not bound: a bound width would undo the compositor
    // every time it placed the window, which is a fight rather than a default.
    onVisibleChanged: {
      if (!visible) {
        root.windowOpen = false
        return
      }
      popped.width = root.paneWidth
      popped.height = root.paneHeight
      // Picked when the window shows — once — and kept thereafter: a standing
      // window is a place you left the cursor, not a fresh question.
      windowView.resetSelection()
      // The window's own copy of the view has to be told to take focus, or its
      // keys go nowhere: nothing else in a bare window claims it.
      Qt.callLater(function(){ windowView.forceActiveFocus() })
    }

    AgentsView {
      id: windowView

      anchors.fill: parent
      anchors.margins: root.contentMargin
      agents: agents
      poppedOut: true
      floating: root.windowFloating
      focus: true
      onCloseRequested: {}
      onActivated: function(entry) { root.goTo(entry) }
      foreground: root.foreground
      fontFamily: root.fontFamily
      contentSpacing: root.contentSpacing
    }
  }
}
