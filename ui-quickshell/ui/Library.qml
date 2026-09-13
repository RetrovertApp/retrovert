import QtQuick
import Retrovert
import "."

// The library screen, ui-ref/retrovert-2a-library.png: rail, table, module panel over a
// transport bar. Everything on it reads from one Session: the rail and table from its
// library document, the panel from its track and queue documents, the transport from its
// per-frame state. The rail's selection and the table's search and sort live here, the one
// place both can see them.
Rectangle {
    id: root
    required property Session session
    // Empty means every format, every collection.
    property string formatFilter: ""
    property string collectionFilter: ""
    property string search: ""
    property string sortKey: "added"
    // Passed up from the transport's channel meter.
    signal meterClicked()

    color: Theme.color.background

    Transport {
        id: transport
        anchors.left: parent.left
        anchors.right: parent.right
        anchors.bottom: parent.bottom
        height: 84
        z: 1
        session: root.session
        onMeterClicked: root.meterClicked()
    }
    LibraryRail {
        id: rail
        anchors.left: parent.left
        anchors.top: parent.top
        anchors.bottom: transport.top
        width: 220
        session: root.session
        screen: root
    }
    ModulePanel {
        id: panel
        anchors.right: parent.right
        anchors.top: parent.top
        anchors.bottom: transport.top
        width: 320
        session: root.session
    }
    LibraryTable {
        anchors.left: rail.right
        anchors.right: panel.left
        anchors.top: parent.top
        anchors.bottom: transport.top
        session: root.session
        screen: root
    }
}
