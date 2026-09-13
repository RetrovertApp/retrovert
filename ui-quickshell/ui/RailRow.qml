import QtQuick
import QtQuick.Layouts
import "."

// One entry of the library rail: a label and its count, with the selected row raised and
// marked by a 2 px accent rule on its left edge.
Rectangle {
    id: root
    property string label
    property string count
    property bool selected: false
    signal clicked()

    Layout.fillWidth: true
    implicitHeight: text.implicitHeight + 12
    color: selected ? Theme.color.raised : hover.containsMouse ? Theme.color.background : "transparent"
    Rectangle { width: 2; height: parent.height; color: root.selected ? Theme.color.accent : "transparent" }
    Text {
        id: text
        anchors.left: parent.left
        anchors.leftMargin: 20
        anchors.right: number.left
        anchors.rightMargin: 8
        anchors.verticalCenter: parent.verticalCenter
        text: root.label
        color: root.selected ? Theme.color.foreground : Theme.color.text
        font.family: Theme.fontFamily
        font.pixelSize: Theme.font.subtitle
        elide: Text.ElideRight
    }
    Mono {
        id: number
        anchors.right: parent.right
        anchors.rightMargin: 18
        anchors.verticalCenter: parent.verticalCenter
        text: root.count
        color: Theme.color.muted
    }
    MouseArea {
        id: hover
        anchors.fill: parent
        hoverEnabled: true
        cursorShape: Qt.PointingHandCursor
        onClicked: root.clicked()
    }
}
