// amon's own — no upstream counterpart.
//
// The amon mark: head, ears, antenna. Same geometry as website/amon-logo.svg,
// which was traced from the source raster and verified against it — so this and
// the site draw one logo rather than two that resemble each other.
//
// The wordmark is deliberately not here. In a hero the name is set in the
// panel's own title face beside this, and a mark carrying its own "amon" would
// say it twice in two typefaces.
//
// Vector rather than an <Image> of the SVG: Omarchy's own plugin icons avoid
// Qt's SVG loader (see TailscaleIcon, DropboxIcon), and `fill="currentColor"`
// could not follow the theme through it anyway. QtQuick.Shapes is what Dropbox
// uses for the same job.

import QtQuick
import QtQuick.Shapes
import qs.Commons

Item {
  id: root

  property real iconSize: Style.font.icon
  property color color: Color.foreground

  // The mark's own bounding box in the SVG's coordinates: the ears set the
  // width, the antenna's crown and the chin set the height. Coordinates below
  // are the SVG's untouched, and this box is what maps them onto `iconSize`.
  readonly property real sourceLeft: 33.75
  readonly property real sourceTop: 16
  readonly property real sourceWidth: 194.5
  readonly property real sourceHeight: 183.5

  // Height is what `iconSize` means — it is a font size, and every icon beside
  // this one is that tall. The mark is a little wider than it is tall, so the
  // width follows from the aspect rather than being squared off.
  readonly property real unit: iconSize / sourceHeight

  implicitWidth: sourceWidth * unit
  implicitHeight: iconSize
  width: implicitWidth
  height: implicitHeight

  Shape {
    // Drawn in the SVG's coordinates and scaled as a whole, so the numbers
    // below can be read against the SVG line for line. The offset cancels the
    // bounding box's origin, putting the mark's top-left at this item's.
    x: -root.sourceLeft * root.unit
    y: -root.sourceTop * root.unit
    // In source units, like everything inside: large enough to contain the
    // drawing, since the layer texture is cut to this box.
    width: 262
    height: 210
    transform: Scale { xScale: root.unit; yScale: root.unit }

    antialiasing: true
    // Rendered at source size and scaled down — roughly eight times the final
    // pixels, which is what keeps a 24px mark's curves clean.
    layer.enabled: true
    layer.smooth: true
    layer.samples: 4

    // Head, with the eyes knocked out of the same path by the even-odd rule.
    // The outline is a superellipse, not a rounded rectangle: eight cubics
    // approximating |x/74.5|^4.4 + |y/66.5|^4.4 = 1 about (131,133).
    ShapePath {
      fillColor: root.color
      strokeWidth: 0
      fillRule: ShapePath.OddEvenFill
      PathSvg {
        path: "M 205.5 133 C 205.5 163.75 205.56 180.06 194.64 189.81 C 182.77 200.41 159.67 199.5 131 199.5 C 102.33 199.5 79.23 200.41 67.36 189.81 C 56.44 180.06 56.5 163.75 56.5 133 C 56.5 102.25 56.44 85.94 67.36 76.19 C 79.23 65.59 102.33 66.5 131 66.5 C 159.67 66.5 182.77 65.59 194.64 76.19 C 205.56 85.94 205.5 102.25 205.5 133 Z M 96.5 122.75 A 8 8 0 0 1 112.5 122.75 L 112.5 143.25 A 8 8 0 0 1 96.5 143.25 Z M 149.5 122.75 A 8 8 0 0 1 165.5 122.75 L 165.5 143.25 A 8 8 0 0 1 149.5 143.25 Z"
      }
    }

    // Ears: capsules floating clear of the head, not attached to it.
    ShapePath {
      fillColor: root.color
      strokeWidth: 0
      PathSvg {
        path: "M 33.75 120.5 A 7.25 7.25 0 0 1 48.25 120.5 L 48.25 145.5 A 7.25 7.25 0 0 1 33.75 145.5 Z M 213.75 120.5 A 7.25 7.25 0 0 1 228.25 120.5 L 228.25 145.5 A 7.25 7.25 0 0 1 213.75 145.5 Z"
      }
    }

    // Antenna ball.
    ShapePath {
      fillColor: root.color
      strokeWidth: 0
      PathSvg { path: "M 118 29 A 13 13 0 1 1 144 29 A 13 13 0 1 1 118 29 Z" }
    }

    // Antenna stem, stopping short of the head the way the ears do. Stroked
    // rather than filled, so the round caps come from the pen as they do in
    // the SVG.
    ShapePath {
      strokeColor: root.color
      fillColor: "transparent"
      strokeWidth: 7
      capStyle: ShapePath.RoundCap
      PathSvg { path: "M 131 42 L 131 56" }
    }
  }
}
