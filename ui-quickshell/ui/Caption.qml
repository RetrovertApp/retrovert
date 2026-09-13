import QtQuick
import "."

// A section label: small, medium weight, letter-spaced, muted.
Text {
    property real tracking: 0.14
    color: Theme.color.muted
    font.family: Theme.fontFamily
    font.pixelSize: Theme.font.caption
    font.weight: Font.Medium
    font.letterSpacing: Theme.font.caption * tracking
}
