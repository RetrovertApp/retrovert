import QtQuick
import "."

// The bordered format badge in the table.
Rectangle {
    property string fmt
    implicitWidth: 44
    implicitHeight: label.implicitHeight + 4
    color: "transparent"
    border.width: 1
    border.color: Theme.color.line
    Text {
        id: label
        anchors.centerIn: parent
        text: parent.fmt
        color: Theme.formatColor(parent.fmt)
        font.family: Theme.fontFamily
        font.pixelSize: Theme.font.caption
    }
}
