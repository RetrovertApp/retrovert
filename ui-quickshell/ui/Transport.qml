import QtQuick
import QtQuick.Layouts
import Retrovert
import "."

// The bar along the foot of the window: channel meter and title, transport, output and volume.
// The progress rule sits on its top edge and seeks where it is clicked.
Rectangle {
    id: root
    required property Session session
    // The channel meter was clicked: the shell swaps between the library and the pattern view.
    signal meterClicked()

    readonly property var track: session.track
    readonly property bool hasTrack: track.title !== undefined
    readonly property bool playing: session.status === "playing"
    readonly property string title: session.status === "error" && session.error.length > 0 ? session.error
                                  : hasTrack ? track.title
                                  : session.status === "idle" ? "no song" : ""
    readonly property string subtitle: session.status === "idle" ? "demo waveform"
                                     : !hasTrack ? ""
                                     : (track.composer ? track.composer + " · " : "") + track.format
                                       + " " + track.channels + "ch · " + track.plugin

    function hex(n) { return String(n.toString(16).toUpperCase()).padStart(2, "0") }
    function toggle() {
        if (session.status === "finished" && hasTrack) session.open(track.path)
        else session.togglePause()
    }

    color: Theme.color.surface

    // Progress rule, with a taller hit area than its two pixels.
    Rectangle {
        width: parent.width
        height: 2
        color: Theme.color.line
        Rectangle {
            height: parent.height
            width: root.session.durationMs > 0 ? parent.width * Math.min(1, root.session.positionMs / root.session.durationMs) : 0
            color: Theme.color.accent
        }
        MouseArea {
            anchors.left: parent.left
            anchors.right: parent.right
            anchors.verticalCenter: parent.verticalCenter
            height: 12
            cursorShape: root.session.durationMs > 0 ? Qt.PointingHandCursor : Qt.ArrowCursor
            onClicked: function (mouse) {
                if (root.session.durationMs > 0) root.session.seek(Math.round(mouse.x / width * root.session.durationMs))
            }
        }
    }

    // Now playing.
    RowLayout {
        anchors.left: parent.left
        anchors.leftMargin: 18
        anchors.verticalCenter: parent.verticalCenter
        width: 320
        spacing: 12
        Rectangle {
            implicitWidth: 52
            implicitHeight: 52
            color: Theme.color.background
            border.width: 1
            border.color: Theme.color.line
            Vu {
                anchors.fill: parent
                anchors.margins: 5
                session: root.session
                columns: 4
                gap: 2
                color: Theme.color.accent
            }
            MouseArea { anchors.fill: parent; cursorShape: Qt.PointingHandCursor; onClicked: root.meterClicked() }
        }
        ColumnLayout {
            Layout.fillWidth: true
            spacing: 3
            Text {
                Layout.fillWidth: true
                text: root.title
                color: root.session.status === "error" ? Theme.color.red : Theme.color.foreground
                font.family: Theme.fontFamily
                font.pixelSize: Theme.font.subtitle
                font.weight: Font.DemiBold
                elide: Text.ElideRight
            }
            Mono { Layout.fillWidth: true; text: root.subtitle; font.pixelSize: Theme.font.body }
        }
    }

    // Transport.
    ColumnLayout {
        anchors.centerIn: parent
        spacing: 8
        Row {
            Layout.alignment: Qt.AlignHCenter
            spacing: 22
            Row {
                anchors.verticalCenter: parent.verticalCenter
                spacing: 4
                readonly property bool many: root.session.subsongCount > 1
                Mono { text: "sub"; color: Theme.color.muted }
                Mono {
                    text: "‹"; color: parent.many && root.session.subsong > 0 ? Theme.color.text : Theme.color.muted
                    MouseArea { anchors.fill: parent; anchors.margins: -4; cursorShape: Qt.PointingHandCursor
                                onClicked: root.session.setSubsong(root.session.subsong - 1) }
                }
                Mono {
                    text: root.session.subsongCount > 0 ? (root.session.subsong + 1) + "/" + root.session.subsongCount : "–"
                    color: Theme.color.muted
                }
                Mono {
                    text: "›"; color: parent.many && root.session.subsong + 1 < root.session.subsongCount ? Theme.color.text : Theme.color.muted
                    MouseArea { anchors.fill: parent; anchors.margins: -4; cursorShape: Qt.PointingHandCursor
                                onClicked: root.session.setSubsong(root.session.subsong + 1) }
                }
            }
            Mono {
                anchors.verticalCenter: parent.verticalCenter; text: "◀◀"; font.pixelSize: Theme.font.title
                MouseArea { anchors.fill: parent; anchors.margins: -6; cursorShape: Qt.PointingHandCursor; onClicked: root.session.previous() }
            }
            Rectangle {
                width: 36
                height: 36
                color: Theme.color.foreground
                Mono {
                    anchors.centerIn: parent
                    text: root.playing ? "❚❚" : "▶"
                    color: Theme.color.background
                    font.pixelSize: Theme.font.body
                }
                MouseArea { anchors.fill: parent; cursorShape: Qt.PointingHandCursor; onClicked: root.toggle() }
            }
            Mono {
                anchors.verticalCenter: parent.verticalCenter; text: "▶▶"; font.pixelSize: Theme.font.title
                MouseArea { anchors.fill: parent; anchors.margins: -6; cursorShape: Qt.PointingHandCursor; onClicked: root.session.next() }
            }
            Mono {
                anchors.verticalCenter: parent.verticalCenter
                text: root.session.loopMode === 2 ? "loop ↻ all" : root.session.loopMode === 1 ? "loop ↻ one" : "loop off"
                color: root.session.loopMode === 0 ? Theme.color.muted : Theme.color.text
                MouseArea { anchors.fill: parent; anchors.margins: -4; cursorShape: Qt.PointingHandCursor
                            onClicked: root.session.setLoop((root.session.loopMode + 1) % 3) }
            }
        }
        Text {
            Layout.alignment: Qt.AlignHCenter
            textFormat: Text.StyledText
            color: Theme.color.muted
            font.family: Theme.fontFamily
            font.pixelSize: Theme.font.small
            readonly property string strong: "<font color=\"" + Theme.color.text + "\">"
            text: strong + Theme.clock(root.session.positionMs) + "</font>"
                  + (root.session.durationMs > 0 ? " / " + Theme.clock(root.session.durationMs) : "")
                  + (root.session.hasPosition
                     ? " &nbsp;·&nbsp; ord " + strong + root.session.order + "</font>"
                       + " &nbsp;pat " + strong + root.hex(root.session.pattern) + "</font>"
                       + " &nbsp;row " + strong + root.hex(root.session.row) + "</font>"
                     : "")
        }
    }

    // Output and volume.
    Row {
        anchors.right: parent.right
        anchors.rightMargin: 18
        anchors.verticalCenter: parent.verticalCenter
        spacing: 16
        Rectangle {
            anchors.verticalCenter: parent.verticalCenter
            width: output.implicitWidth + 12
            height: output.implicitHeight + 6
            color: "transparent"
            border.width: 1
            border.color: Theme.color.line
            Mono { id: output; anchors.centerIn: parent; text: Math.round(root.session.sampleRate / 1000) + "k · stereo" }
        }
        Mono { anchors.verticalCenter: parent.verticalCenter; text: "vol"; color: Theme.color.muted }
        Rectangle {
            id: slider
            anchors.verticalCenter: parent.verticalCenter
            width: 100
            height: 2
            color: Theme.color.line
            Rectangle { width: parent.width * root.session.volume; height: parent.height; color: Theme.color.text }
            MouseArea {
                anchors.left: parent.left
                anchors.right: parent.right
                anchors.verticalCenter: parent.verticalCenter
                height: 20
                cursorShape: Qt.PointingHandCursor
                function set(x) { root.session.setVolume(Math.max(0, Math.min(1, x / slider.width))) }
                onPressed: function (mouse) { set(mouse.x) }
                onPositionChanged: function (mouse) { if (pressed) set(mouse.x) }
                onWheel: function (wheel) {
                    root.session.setVolume(Math.max(0, Math.min(1, root.session.volume + (wheel.angleDelta.y > 0 ? 0.05 : -0.05))))
                }
            }
        }
        Mono { anchors.verticalCenter: parent.verticalCenter; text: Math.round(root.session.volume * 100) }
    }
}
