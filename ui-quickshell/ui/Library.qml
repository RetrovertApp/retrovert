import QtQuick
import "."

// The library screen, ui-ref/retrovert-2a-library.png: rail, table, module panel over a
// transport bar. Content is placeholder until the catalog and playlist feed it; what the
// engine already knows (title, plugin, position) is passed in from shell.qml.
Rectangle {
    id: root
    property alias title: transport.title
    property alias subtitle: transport.subtitle
    property alias elapsedMs: transport.elapsedMs
    property alias durationMs: transport.durationMs
    property alias playing: transport.playing
    property alias status: panel.status

    color: Theme.color.background

    Transport {
        id: transport
        anchors.left: parent.left
        anchors.right: parent.right
        anchors.bottom: parent.bottom
        height: 84
        z: 1
    }
    LibraryRail {
        id: rail
        anchors.left: parent.left
        anchors.top: parent.top
        anchors.bottom: transport.top
        width: 220
    }
    ModulePanel {
        id: panel
        anchors.right: parent.right
        anchors.top: parent.top
        anchors.bottom: transport.top
        width: 320
        title: transport.title
    }
    LibraryTable {
        anchors.left: rail.right
        anchors.right: panel.left
        anchors.top: parent.top
        anchors.bottom: transport.top
    }
}
