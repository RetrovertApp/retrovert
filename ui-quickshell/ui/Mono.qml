import QtQuick
import "."

// Body text in the desktop font, the default size and colour of the design's small print.
Text {
    color: Theme.color.text
    font.family: Theme.fontFamily
    font.pixelSize: Theme.font.small
    elide: Text.ElideRight
}
