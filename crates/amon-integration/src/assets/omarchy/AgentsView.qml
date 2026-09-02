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
import Quickshell.Io
import qs.Commons
import qs.Ui

// A FocusScope and not an Item: PanelKeyCatcher below declares `focus: true`,
// and inside a plain Item that does nothing — the host would take active focus
// here and the catcher would never see a key. Measured, not guessed: with an
// Item the arrows and j/k were dead while Ctrl-N still worked, because the
// Ctrl chords are handled on this object and the rest are handled on the child.
FocusScope {
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

  // Escape. Only the host knows whether there is anything to dismiss.
  signal closeRequested()

  // Which row the cursor is on. There is always one while there are rows at
  // all: a list you arrow into from nowhere makes you press a key to find out
  // where you are.
  property int selectedIndex: 0

  // Back to where the cursor should start, asked of the model on every way
  // in. The rule is the model's (`initialIndex`) because it is about agents
  // and workspaces, not about drawing them — the view only owns which row the
  // cursor is on, not where it belongs.
  function resetSelection() {
    view.selectedIndex = view.agents.initialIndex()
  }

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

  // Ctrl-N and Ctrl-P, which PanelKeyCatcher does not know: Omarchy's component
  // covers the arrows and vi's j/k, and stops there.
  //
  // Handled on the way past rather than inside the catcher. A Ctrl chord's
  // `event.text` is a control character, not a letter, so the catcher's
  // text hook cannot recognise it — but the catcher never accepts those events
  // either, so they propagate out of the focused child and reach here.
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

  // Column widths come from measuring the actual strings. Counting characters
  // and multiplying by one advance is close enough for ASCII and wrong for
  // everything else: a monospaced font still draws CJK at two cells, and a
  // repository named in Chinese would be allotted half the room it needs —
  // and the identifying segment is never elided, so it would run into the
  // column beside it rather than being cut short.
  //
  // `advanceWidth` is a call rather than a property, which is what keeps this
  // usable from inside a binding: measuring a string here cannot invalidate
  // the binding that asked for it.
  FontMetrics {
    id: metrics
    font.family: view.fontFamily
    font.pixelSize: Style.font.body
  }
  function textWidth(text) {
    return text ? Math.ceil(metrics.advanceWidth(text)) : 0
  }

  readonly property int columnGap: Style.space(10)

  // Left to right. The age sits at the far edge, where a column of short
  // right-aligned values reads down cleanly, and the message runs between the
  // branch and it.
  readonly property var columnOrder: ["glyph", "identity", "branch", "activity", "age"]

  // Only the branch. Every other column either identifies a row or gives way
  // by shrinking, and a pane too narrow for the branch has already given up
  // the message.
  readonly property var droppable: ["branch"]

  // Below this the message is more ellipsis than words, and the space reads
  // better as nothing at all.
  readonly property int activityMinimum: view.textWidth("m".repeat(10))

  // A little air before the message, so a sentence does not start hard against
  // the branch the way a short fixed value can. Taken out of the column's own
  // width rather than added to the row, which keeps the age against the edge.
  readonly property int activityInset: Style.space(4)

  // A band of light crossing the message while an agent is working.
  //
  // One phase drives every row, and the band is measured against the message
  // *column* rather than against each row's own sentence — the column is one
  // width for the whole list, so the highlight is at the same place on every
  // working row at the same instant. What crosses the pane is one wave, not
  // several rows each animating on their own clock. A short message simply
  // stops being lit sooner than a long one, which is what a single wave
  // passing over text of different lengths looks like.
  //
  // Nothing moves: the glyphs are fixed and only their colour travels, which
  // is what keeps this readable rather than distracting. The colours are the
  // row's own two tokens, so the effect follows any theme instead of naming
  // colours of its own.
  // The shimmer crosses quickly and then the row rests. Named as two spans
  // rather than as a period the sweep is subtracted from, because the wait is
  // the thing being chosen — a band drawn out to fill a period would not read
  // as a shimmer at all, it would read as text slowly changing colour. 900ms
  // is a duration the shell already animates at elsewhere.
  readonly property int shimmerSweep: 900
  readonly property int shimmerRest: 2000

  // Parked at 1, which is the band already past the right edge: nothing is lit
  // between shimmers, and nothing is lit before the first one.
  property real shimmerPhase: 1
  property bool shimmerRunning: false
  property bool sweeping: false

  // Raised as each shimmer finishes. A row that has stopped working waits for
  // this rather than dropping out under a band that is still crossing it.
  signal sweepFinished()

  readonly property bool anyWorking: {
    const rows = view.agents.rows
    for (let i = 0; i < rows.length; i++) {
      if (rows[i].state === "working") return true
    }
    return false
  }

  onAnyWorkingChanged: {
    if (view.anyWorking) view.shimmerRunning = true
    else if (!view.sweeping) view.shimmerRunning = false
  }
  Component.onCompleted: if (view.anyWorking) view.shimmerRunning = true

  // Stopped outright when no agent is working, so an idle pane costs nothing —
  // but never mid-shimmer: the run ends where a band has finished crossing.
  SequentialAnimation {
    running: view.shimmerRunning
    loops: Animation.Infinite

    ScriptAction { script: view.sweeping = true }
    NumberAnimation {
      target: view
      property: "shimmerPhase"
      from: 0
      to: 1
      duration: view.shimmerSweep
    }
    ScriptAction {
      script: {
        view.sweeping = false
        view.sweepFinished()
        if (!view.anyWorking) view.shimmerRunning = false
      }
    }
    PauseAnimation { duration: view.shimmerRest }
  }

  // The band's shape, in characters. It is flat across the peak and falls off
  // either side — a single cosine bump, which is what this was, only ever
  // reaches full brightness at one point, so the light read as a moving dot
  // rather than as a lit word.
  //
  // Flat top plus cosine shoulders is a Tukey window. The plateau is what you
  // set when you want "three characters lit"; the shoulders are what keep it
  // from looking like a rectangle sliding past.
  readonly property int shimmerPeakChars: 3
  readonly property int shimmerFalloffChars: 8

  // One character per piece, because a three-character plateau cannot be drawn
  // by pieces five characters wide — the band would land inside a single piece
  // and light all of it. The font is monospace, so a piece per character costs
  // nothing in layout: every glyph is one advance and the Row rebuilds the same
  // line the plain Text would have drawn.
  readonly property int shimmerChunkChars: 1

  readonly property real shimmerCharWidth: Math.max(1, view.textWidth("m"))
  readonly property real shimmerPeak: view.shimmerPeakChars * view.shimmerCharWidth
  readonly property real shimmerFalloff: view.shimmerFalloffChars * view.shimmerCharWidth
  readonly property real shimmerReach: view.shimmerPeak / 2 + view.shimmerFalloff

  // How lit a character is: 1 across the plateau, a cosine shoulder down to 0,
  // nothing beyond. Distance is measured in the message column's own
  // coordinates, which every working row shares — that is what puts them in
  // step.
  function shimmerAt(centerX) {
    const half = view.shimmerPeak / 2
    const falloff = view.shimmerFalloff
    // Starts and ends off the column, so the band enters and leaves rather
    // than appearing mid-message.
    const head = -view.shimmerReach
      + view.shimmerPhase * (view.columnWidth("activity") + 2 * view.shimmerReach)
    const distance = Math.abs(centerX - head)
    if (distance <= half) return 1
    if (distance >= half + falloff || falloff <= 0) return 0
    return (1 + Math.cos((distance - half) / falloff * Math.PI)) / 2
  }

  function shimmerColor(centerX) {
    const lit = view.shimmerAt(centerX)
    if (lit <= 0) return view.dim
    const from = view.dim
    const to = view.foreground
    return Qt.rgba(
      from.r + (to.r - from.r) * lit,
      from.g + (to.g - from.g) * lit,
      from.b + (to.b - from.b) * lit,
      1)
  }

  // The message split into pieces the band can resolve.
  function shimmerChunks(text) {
    if (!text) return []
    const size = Math.max(1, view.shimmerChunkChars)
    const out = []
    for (let i = 0; i < text.length; i += size) out.push(text.slice(i, i + size))
    return out
  }

  // Where the agent is, split into the one segment that identifies it and the
  // qualification around it. Inside a repository that is the Project, and the
  // subpath trails it; outside one there is no Project, and the directory the
  // agent stands in is what you recognise, with the path to it leading up.
  //
  // Only ever one of prefix and suffix is set, which is what lets one layout
  // draw both cases: the bold segment keeps its width and the dim part takes
  // what is left, eliding from whichever side it sits on.
  function identityParts(entry) {
    if (entry.project !== "")
      return {
        prefix: "",
        bold: entry.project,
        suffix: entry.subpath !== "" ? "/" + entry.subpath : ""
      }
    const path = view.shortPath(entry.cwd)
    const cut = path.lastIndexOf("/")
    if (cut < 0)
      return { prefix: "", bold: path, suffix: "" }
    return { prefix: path.slice(0, cut + 1), bold: path.slice(cut + 1), suffix: "" }
  }

  function cellText(entry, column) {
    if (column === "identity") {
      const parts = view.identityParts(entry)
      return parts.prefix + parts.bold + parts.suffix
    }
    if (column === "branch") return entry.branch
    return ""
  }

  // One set of column widths for the whole visible list, so a column starts at
  // the same place on every line and the eye can run down it. herdr packs each
  // row independently, which suits a list of sentences; this is a grid, and a
  // grid whose columns move per row is not one.
  //
  // A column no row can fill is not drawn at all — a branch column where
  // nothing is in a repository, for instance.
  function computeColumns(rows, available) {
    const gap = view.columnGap
    const natural = {
      glyph: Style.space(18),
      identity: 0,
      branch: 0,
      activity: 0,
      // Sized for the longest age this column can hold rather than for the
      // one showing now, so the row does not shuffle when 59s becomes 1m.
      age: view.textWidth("9999h")
    }

    for (let i = 0; i < rows.length; i++) {
      const entry = rows[i]
      natural.identity = Math.max(natural.identity, view.textWidth(view.cellText(entry, "identity")))
      natural.branch = Math.max(natural.branch, view.textWidth(entry.branch))
      natural.activity = Math.max(natural.activity, view.textWidth(entry.activity))
    }

    let present = view.columnOrder.filter(column => natural[column] > 0)
    const width = {}
    for (let i = 0; i < present.length; i++) width[present[i]] = natural[present[i]]

    // Every column but the message keeps its natural width. The message takes
    // whatever is left and elides into it, which is why nothing else has to be
    // dropped to make a pane fit: the slack has somewhere to go, and a long
    // sentence costs its own tail rather than a column that identifies a row.
    while (true) {
      const gaps = Math.max(0, present.length - 1) * gap
      let fixed = 0
      for (let i = 0; i < present.length; i++) {
        if (present[i] !== "activity") fixed += width[present[i]]
      }

      const carries = present.indexOf("activity")
      if (carries >= 0) {
        const slack = available - gaps - fixed
        if (slack >= view.activityMinimum) {
          // All of it, not just what the longest message needs. Taking the
          // slack is what puts the age against the pane's edge, and a message
          // shorter than its column simply does not fill it.
          width.activity = slack
          break
        }
        // Not enough room left to read one. Take it out and let the columns
        // that identify a row use the space.
        present.splice(carries, 1)
        continue
      }

      if (fixed + gaps <= available) break

      let dropped = false
      for (let i = present.length - 1; i >= 0; i--) {
        if (view.droppable.indexOf(present[i]) >= 0) {
          present.splice(i, 1)
          dropped = true
          break
        }
      }
      // Nothing left that may go. The identity takes what remains and elides
      // its dim half; the bold segment is never truncated away.
      if (!dropped) {
        let others = 0
        for (let i = 0; i < present.length; i++) {
          if (present[i] !== "identity") others += width[present[i]]
        }
        width.identity = Math.max(0, available - gaps - others)
        break
      }
    }

    const x = {}
    let cursor = 0
    for (let i = 0; i < present.length; i++) {
      x[present[i]] = cursor
      cursor += width[present[i]] + gap
    }
    // The age belongs at the edge whether or not a message pushed it there, so
    // that a list with nothing to narrate still reads down the same way. When
    // the message did take the slack this changes nothing.
    if (present.indexOf("age") >= 0) {
      x.age = Math.max(x.age, available - width.age)
    }
    return { present: present, width: width, x: x }
  }

  readonly property var columnLayout: view.computeColumns(
    view.agents.rows,
    Math.max(0, list.width - Style.space(10) * 2))

  function columnWidth(column) {
    return view.columnLayout.width[column] || 0
  }
  function columnX(column) {
    return view.columnLayout.x[column] || 0
  }
  function columnVisible(column) {
    return view.columnLayout.present.indexOf(column) >= 0
  }

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
      // say otherwise. Super+O unpins and tiles it — it does not close it, and
      // does not bring the modal back.
      iconText: "󰹙"
      // The key that does this, marked by underlining it — the convention for a
      // mnemonic in a label, and quieter than brackets, which interrupt the
      // word to say the same thing. Omarchy has neither convention: its own
      // panels carry bare-letter shortcuts that appear nowhere on screen, which
      // is why nobody knows the Tailscale panel copies an IP on `c`. Showing
      // the key at all is the departure; how it is shown is just taste.
      //
      // Markup, because underlining part of a string is not something a font
      // property can do. Button's label sets no `textFormat`, so it is AutoText
      // and renders this.
      text: "<u>F</u>loat"
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

  // Omarchy's own key dispatcher, so this pane answers the arrows and vi's
  // keys exactly as every other panel on the desktop does — and keeps doing so
  // if that set ever grows. It lives in the view rather than in either host, so
  // the modal and the popped-out window navigate identically.
  PanelKeyCatcher {
    anchors.fill: parent

    onMoveRequested: function(dx, dy) { if (dy !== 0) view.moveSelection(dy) }
    onActivateRequested: view.activateSelection()
    onCloseRequested: view.closeRequested()
    onTextKey: function(text) {
      if ((text === "f" || text === "F") && !view.poppedOut) view.popOutRequested()
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

    // Everything the header leaves. Measured from the pieces above rather than
    // from this item's own content, which would be a loop: the column would
    // size to its children and the list to the column.
    Item {
      id: body

      width: parent.width
      height: Math.max(0, view.height - hero.height - rule.height - view.contentSpacing * 2
                         - (updateFooter.visible ? updateFooter.height + view.contentSpacing : 0))

    // One flat list with the workspace headings folded into it, the way the
    // network panel separates known networks from the others. A ListView rather
    // than a Repeater in a Column, for the same reason it uses one:
    // `positionViewAtIndex` is what keeps the cursor on screen as it walks past
    // the bottom of the visible window.
    ListView {
      id: list

      anchors.fill: parent
      visible: view.agents.rows.length > 0
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

    // Nothing to list. Centred in the same region the list would fill, so the
    // pane keeps its shape whether or not there is anything in it.
    //
    // "No agents" and not "no agents running": `running` is a state a row can
    // be in, and one word meaning two things in one pane is how you end up
    // wondering whether an idle agent counts. It is the same phrase the header
    // above already uses, so the two lines agree rather than each inventing a
    // way to say it.
    Column {
      visible: view.agents.rows.length === 0
      anchors.centerIn: parent
      width: Math.min(parent.width - Style.space(48), Style.space(380))
      spacing: Style.space(10)

      Text {
        width: parent.width
        horizontalAlignment: Text.AlignHCenter
        text: "No agents"
        color: view.foreground
        font.family: view.fontFamily
        font.pixelSize: Style.font.subtitle
        font.bold: true
      }

      Text {
        width: parent.width
        horizontalAlignment: Text.AlignHCenter
        wrapMode: Text.WordWrap
        // Both ways in: the one you already have open, and the one that needs
        // no terminal. The chord is Omarchy's own — `SUPER + SHIFT + CTRL + A`,
        // bound to `omarchy-agent --pick`, which launches your default agent
        // and offers the picker when you have not chosen one.
        text: "Start one in a terminal, or press Super+Shift+Ctrl+A to launch the default."
        color: view.dim
        font.family: view.fontFamily
        font.pixelSize: Style.font.body
        lineHeight: 1.35
      }
    }
    }

    // The one line the daemon's release check buys: which versions, and the
    // command that closes the gap. Invisible when current — an up-to-date
    // pane owes nobody a footer. Clicking copies the command (wl-copy is
    // stock Omarchy; the clipboard plugin is built on it), because the next
    // stop is a terminal and retyping a pipeline is how typos ship.
    Item {
      id: updateFooter

      // Held to install.sh by a test: the pane must never advertise a line
      // the installer stopped answering to.
      readonly property string command: "curl -fsSL amon.sh/install | sh"
      property bool copied: false

      width: parent.width
      height: footerText.implicitHeight
      visible: view.agents.updateAvailable

      Text {
        id: footerText
        width: parent.width
        horizontalAlignment: Text.AlignHCenter
        elide: Text.ElideRight
        // Material Design's content-copy and check, the family every other
        // icon in this shell comes from. The copy glyph is what says the line
        // is clickable at all — dim text alone reads as a notice, not a
        // control — and the check is the same answer the website's copy
        // buttons give.
        text: updateFooter.copied
          ? "󰄬 copied"
          : "v" + view.agents.installedVersion + " -> v" + view.agents.latestVersion
            + " · " + updateFooter.command + " 󰆏"
        color: view.dim
        font.family: view.fontFamily
        font.pixelSize: Style.font.body
      }

      MouseArea {
        anchors.fill: parent
        cursorShape: Qt.PointingHandCursor
        onClicked: {
          copyCommand.running = true
          updateFooter.copied = true
          copiedFlash.restart()
        }
      }

      // Long enough to read, short enough that the line is back before the
      // hand reaches the terminal.
      Timer {
        id: copiedFlash
        interval: 1500
        onTriggered: updateFooter.copied = false
      }

      Process {
        id: copyCommand
        running: false
        command: ["wl-copy", updateFooter.command]
      }
    }
  }
  }

  // One agent, drawn into the shared column layout so every row agrees where a
  // column starts. What each column holds and when it gives way is decided in
  // `computeColumns` above; a row only places what it is given.
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
    // and the column keeps its width so the rows stay aligned. Never dropped:
    // whether an agent wants you is the one thing a row must always say.
    Text {
      id: stateGlyph
      x: Style.space(10) + view.columnX("glyph")
      anchors.verticalCenter: parent.verticalCenter
      width: view.columnWidth("glyph")
      text: {
        if (row.entry.state === "blocked") return view.agents.blockedGlyph
        if (row.entry.state === "done") return view.agents.doneGlyph
        if (row.entry.state === "working") return view.agents.spinner
        return ""
      }
      color: view.foreground
      font.family: view.fontFamily
      font.pixelSize: Style.font.body
    }

    // Where the agent is working, and the first thing the eye lands on. Bold
    // marks the one segment that identifies it — the Project, or the directory
    // when there is no Project — and that segment is never truncated: elision
    // eats the dim qualification, from whichever side it sits on.
    Item {
      id: identity
      x: Style.space(10) + view.columnX("identity")
      anchors.verticalCenter: parent.verticalCenter
      width: view.columnWidth("identity")
      height: parent.height

      readonly property var parts: view.identityParts(row.entry)
      readonly property real boldWidth: Math.min(view.textWidth(parts.bold), width)

      Text {
        visible: identity.parts.prefix !== ""
        x: 0
        width: Math.max(0, identity.width - identity.boldWidth)
        anchors.verticalCenter: parent.verticalCenter
        text: identity.parts.prefix
        color: view.dim
        font.family: view.fontFamily
        font.pixelSize: Style.font.body
        // The path leading up to the directory: its tail is what matters, so
        // it gives way from the front.
        elide: Text.ElideLeft
        horizontalAlignment: Text.AlignRight
      }

      Text {
        x: identity.parts.prefix !== "" ? Math.max(0, identity.width - identity.boldWidth) : 0
        width: identity.boldWidth
        anchors.verticalCenter: parent.verticalCenter
        text: identity.parts.bold
        color: view.foreground
        font.family: view.fontFamily
        font.pixelSize: Style.font.body
        font.bold: true
      }

      Text {
        visible: identity.parts.suffix !== ""
        x: identity.boldWidth
        width: Math.max(0, identity.width - identity.boldWidth)
        anchors.verticalCenter: parent.verticalCenter
        text: identity.parts.suffix
        color: view.dim
        font.family: view.fontFamily
        font.pixelSize: Style.font.body
        elide: Text.ElideRight
      }
    }

    // Identity too, at a finer grain than the Project. A repository with six
    // worktrees puts the same Project on every row, and then the branch is the
    // only thing telling them apart — so it is drawn at full weight rather
    // than dimmed down with the metadata beside it. Three tiers, each meaning
    // something: bold is which project, plain is which line of work, dim is
    // everything that describes the row rather than identifies it.
    //
    // Not bold, though. Two bold columns compete and neither leads.
    //
    // Blank is the ordinary case for anything not in a repository.
    Text {
      id: branchText
      visible: view.columnVisible("branch")
      x: Style.space(10) + view.columnX("branch")
      anchors.verticalCenter: parent.verticalCenter
      width: view.columnWidth("branch")
      text: row.entry.branch
      color: view.foreground
      font.family: view.fontFamily
      font.pixelSize: Style.font.body
      elide: Text.ElideRight
    }

    // What the agent says it is doing, in the harness's own words — "Reading 5
    // files", "Bash(cargo test)", the opening line of a reply. Never a phrase
    // amon composed: a column that said "Working…" would repeat the glyph
    // beside it, which is the mistake ADR-0017 records.
    //
    // The elastic column. It takes whatever the fixed ones leave and elides
    // into it, so a narrow pane costs this column its tail rather than costing
    // a row something that identifies it.
    //
    // Drawn two ways. A working agent's message is split into chunks so a band
    // of light can cross it; everything else is one plain Text. The split is
    // not the layout — Qt still does the eliding, through the TextMetrics
    // below, and the chunks are cut from the string it hands back. So both
    // paths break the sentence in the same place, with Qt's own ellipsis,
    // whatever font the theme resolves to.
    readonly property int activityWidth:
      Math.max(0, view.columnWidth("activity") - view.activityInset)

    // A prompt is your words, not the agent's, so it wears the harness's own
    // ❯ and leans into italic — the same two cues everywhere the activity is
    // drawn (the plain path, the eliding metrics, the shimmer chunks), so
    // eliding and the shimmer treat marker and text as one run.
    readonly property bool activityIsPrompt: row.entry.activityIsPrompt
    readonly property string activityDisplay:
      row.entry.activity === "" ? ""
        : (row.activityIsPrompt ? "\u276F " + row.entry.activity : row.entry.activity)

    // Whether this row is drawn in chunks so a shimmer can cross it. It is not
    // simply "is working": a row that stops mid-shimmer keeps its chunks until
    // the band has finished crossing, because cutting the light off half way
    // over a sentence is more noticeable than the shimmer itself. Stopping
    // between shimmers takes effect at once, there being nothing to finish.
    readonly property bool working: row.entry.state === "working"
    property bool shimmering: false

    onWorkingChanged: {
      if (row.working) row.shimmering = true
      else if (!view.sweeping) row.shimmering = false
    }
    Component.onCompleted: if (row.working) row.shimmering = true

    Connections {
      target: view
      function onSweepFinished() {
        if (!row.working) row.shimmering = false
      }
    }

    // Qt's elision, asked for rather than left implicit. Reaching for
    // `Math.floor(width / advanceWidth)` instead would be a second answer to a
    // question the other columns already answer one way, and would hold only
    // while the theme's font stayed monospace.
    TextMetrics {
      id: activityMetrics
      font.family: view.fontFamily
      font.pixelSize: Style.font.body
      font.italic: row.activityIsPrompt
      text: row.activityDisplay
      elide: Text.ElideRight
      elideWidth: row.activityWidth
    }

    Text {
      id: activityText
      visible: view.columnVisible("activity") && !row.shimmering
      x: Style.space(10) + view.columnX("activity") + view.activityInset
      anchors.verticalCenter: parent.verticalCenter
      width: row.activityWidth
      text: row.activityDisplay
      color: view.dim
      font.family: view.fontFamily
      font.pixelSize: Style.font.body
      font.italic: row.activityIsPrompt
      elide: Text.ElideRight
    }

    Row {
      id: activityShimmer
      visible: view.columnVisible("activity") && row.shimmering
      x: Style.space(10) + view.columnX("activity") + view.activityInset
      anchors.verticalCenter: parent.verticalCenter
      width: row.activityWidth

      Repeater {
        // Built from the elided string, so the chunks are exactly what the
        // plain Text would have drawn.
        model: activityShimmer.visible ? view.shimmerChunks(activityMetrics.elidedText) : []

        Text {
          text: modelData
          // Measured at the chunk's middle, in the message column's own
          // coordinates — the same coordinates every other working row uses,
          // which is what puts them all in step.
          color: view.shimmerColor(x + width / 2 + view.activityInset)
          font.family: view.fontFamily
          font.pixelSize: Style.font.body
          font.italic: row.activityIsPrompt
        }
      }
    }

    // How long the agent has been in the state it is in — not how long it has
    // been running. A row that has wanted you for forty minutes is a different
    // thing from one that has wanted you for one, and the glyph cannot say so.
    //
    // Last, and right-aligned: these are short values of varying length, and
    // flushing them to the pane's edge makes the column read down as one.
    Text {
      id: ageText
      visible: view.columnVisible("age")
      x: Style.space(10) + view.columnX("age")
      anchors.verticalCenter: parent.verticalCenter
      width: view.columnWidth("age")
      text: view.agents.age(row.entry.stateSince, view.now)
      color: view.dim
      font.family: view.fontFamily
      font.pixelSize: Style.font.body
      horizontalAlignment: Text.AlignRight
      // The column is sized for a very old agent, but age() counts hours
      // without end. Eliding rather than overflowing keeps a row that has been
      // idle for years from drawing over the column beside it.
      elide: Text.ElideRight
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
