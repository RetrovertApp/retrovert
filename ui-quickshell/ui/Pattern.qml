import QtQuick
import QtQuick.Layouts
import Retrovert
import "."

// The pattern screen, ui-ref/retrovert-2b-hero.png with one change: each channel's scope
// sits above its own column instead of in a strip along the foot, so the eye reads a
// channel's notes and its waveform in one place. Top bar, the grid under its scope header,
// the samples panel, and the shared transport. The mockup's order rail is left out because
// the plugin API has no order list to fill it from.
Rectangle {
    id: root
    required property Session session
    signal libraryRequested()
    signal meterClicked()

    readonly property var track: session.track
    readonly property bool hasTrack: track.title !== undefined
    readonly property int channels: session.patternChannels
    readonly property var names: (track.samples || []).length > 0 ? track.samples : (track.instruments || [])
    // A decoder with no real message may hand back the sample list as one; that is shown once.
    readonly property string message: {
        var text = track.message || ""
        var lines = text.split("\n").filter(function (l) { return l.trim().length > 0 })
        var names = root.names.map(function (n) { return n.trim() })
        var fromSamples = lines.length > 0 && lines.every(function (l) { return names.indexOf(l.trim()) >= 0 })
        return fromSamples ? "" : text
    }
    readonly property string facts: !hasTrack ? "" : "· " + track.format + " · " + track.channels + "ch"
                                     + (track.year > 0 ? " · " + track.year : "") + " · " + track.plugin

    function hex(n) { return String(n.toString(16).toUpperCase()).padStart(2, "0") }

    color: Theme.color.background

    // Top bar: wordmark, song, facts, tracker position, the way back.
    Rectangle {
        id: bar
        anchors.left: parent.left
        anchors.right: parent.right
        anchors.top: parent.top
        height: 44
        color: Theme.color.surface
        Rectangle { anchors.bottom: parent.bottom; width: parent.width; height: 1; color: Theme.color.line }
        RowLayout {
            anchors.fill: parent
            anchors.bottomMargin: 1
            spacing: 0
            Rectangle {
                Layout.fillHeight: true
                implicitWidth: mark.implicitWidth + 36
                color: Theme.color.accent
                Caption {
                    id: mark
                    anchors.centerIn: parent
                    text: "RETROVERT"
                    color: Theme.color.background
                    font.pixelSize: Theme.font.body
                    font.weight: Font.DemiBold
                }
            }
            Text {
                Layout.leftMargin: 18
                Layout.rightMargin: 18
                Layout.maximumWidth: 320
                text: root.session.status === "error" && root.session.error.length > 0 ? root.session.error
                    : root.hasTrack ? root.track.title : "no song"
                color: root.session.status === "error" ? Theme.color.red
                     : root.hasTrack ? Theme.color.foreground : Theme.color.muted
                font.family: Theme.fontFamily
                font.pixelSize: Theme.font.title
                font.weight: Font.DemiBold
                elide: Text.ElideRight
            }
            Mono {
                Layout.maximumWidth: 240
                visible: text.length > 0
                text: root.hasTrack ? (root.track.composer || "") : ""
                font.pixelSize: Theme.font.body
            }
            Mono {
                Layout.leftMargin: 18
                Layout.maximumWidth: 360
                text: root.facts
                color: Theme.color.muted
                font.pixelSize: Theme.font.body
            }
            Item { Layout.fillWidth: true }
            Text {
                Layout.rightMargin: 18
                visible: root.session.hasPosition
                textFormat: Text.StyledText
                color: Theme.color.muted
                font.family: Theme.fontFamily
                font.pixelSize: Theme.font.body
                readonly property string strong: "<font color=\"" + Theme.color.foreground + "\">"
                text: "ord " + strong + root.hex(root.session.order) + "</font>"
                    + " &nbsp; pat " + strong + root.hex(root.session.pattern) + "</font>"
                    + " &nbsp; row " + strong + root.hex(root.session.row) + "</font>"
            }
            Rectangle {
                Layout.fillHeight: true
                implicitWidth: esc.implicitWidth + 36
                color: "transparent"
                Rectangle { width: 1; height: parent.height; color: Theme.color.line }
                Mono { id: esc; anchors.centerIn: parent; text: "esc: library"; color: Theme.color.muted; font.pixelSize: Theme.font.body }
                MouseArea { anchors.fill: parent; cursorShape: Qt.PointingHandCursor; onClicked: root.libraryRequested() }
            }
        }
    }

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

    // Samples and the file's message.
    Rectangle {
        id: panel
        anchors.right: parent.right
        anchors.top: bar.bottom
        anchors.bottom: transport.top
        width: 260
        color: Theme.color.surface
        clip: true
        Rectangle { width: 1; height: parent.height; color: Theme.color.line }
        ColumnLayout {
            anchors.fill: parent
            anchors.leftMargin: 1
            anchors.topMargin: 12
            spacing: 0
            Caption {
                Layout.leftMargin: 16
                Layout.bottomMargin: 8
                text: (root.track.samples || []).length > 0 ? "SAMPLES" : "INSTRUMENTS"
            }
            Item {
                Layout.fillWidth: true
                Layout.fillHeight: true
                clip: true
                Column {
                    width: parent.width
                    Repeater {
                        model: root.names
                        Mono {
                            required property string modelData
                            required property int index
                            width: parent.width - 32
                            x: 16
                            height: 22
                            verticalAlignment: Text.AlignVCenter
                            text: String(index + 1).padStart(2, "0") + " " + modelData
                            color: modelData.trim().length > 0 ? Theme.color.text : Theme.color.muted
                        }
                    }
                }
                Rectangle {
                    anchors.fill: parent
                    gradient: Gradient {
                        GradientStop { position: 0.8; color: "transparent" }
                        GradientStop { position: 1.0; color: Theme.color.surface }
                    }
                }
            }
            Item {
                Layout.fillWidth: true
                visible: root.message.length > 0
                implicitHeight: note.implicitHeight + 24
                Rectangle { width: parent.width; height: 1; color: Theme.color.line }
                Mono {
                    id: note
                    x: 16
                    y: 12
                    width: parent.width - 32
                    text: "message:\n" + root.message
                    color: Theme.color.muted
                    wrapMode: Text.Wrap
                    lineHeight: 1.6
                    maximumLineCount: 8
                }
            }
        }
    }

    // The grid, with each channel's scope above its column. The header's cells are laid
    // out from the same channel width the grid divides its width into.
    Item {
        id: centre
        anchors.left: parent.left
        anchors.right: panel.left
        anchors.top: bar.bottom
        anchors.bottom: transport.top
        readonly property real channelWidth: (width - grid.numberWidth) / Math.max(1, root.channels)

        Item {
            id: header
            anchors.left: parent.left
            anchors.right: parent.right
            anchors.top: parent.top
            height: 56
            visible: root.channels > 0
            Repeater {
                model: root.channels
                Item {
                    required property int index
                    x: grid.numberWidth + index * centre.channelWidth
                    width: centre.channelWidth
                    height: header.height
                    Rectangle { width: 1; height: parent.height; color: Theme.color.line }
                    Scope {
                        anchors.fill: parent
                        anchors.leftMargin: 3
                        anchors.rightMargin: 2
                        anchors.topMargin: 3
                        anchors.bottomMargin: 3
                        session: root.session
                        channel: parent.index
                        color: Theme.color.accent
                        lineWidth: 1.2
                    }
                    Mono { x: 6; y: 4; text: root.hex(parent.index + 1); color: Theme.color.muted; font.pixelSize: Theme.font.caption }
                }
            }
            Rectangle { anchors.bottom: parent.bottom; width: parent.width; height: 1; color: Theme.color.line }
        }

        Pattern {
            id: grid
            anchors.left: parent.left
            anchors.right: parent.right
            anchors.top: header.visible ? header.bottom : parent.top
            anchors.bottom: parent.bottom
            clip: true
            session: root.session
            font: Qt.font({ family: Theme.fontFamily, pixelSize: Theme.font.body })
            rowHeight: 26
            numberWidth: 44
            numberPadding: 14
            fade: 0.8
            noteColor: Theme.color.foreground
            noteOffColor: Theme.color.text
            instrumentColor: Theme.color.yellow
            volumeColor: Theme.color.green
            effectColor: Theme.color.accent2
            paramColor: Theme.color.text
            emptyColor: Theme.color.line
            numberColor: Theme.color.muted
            cursorColor: Theme.color.raised
            cursorLineColor: Theme.color.accent
            lineColor: Theme.color.line
        }

        Mono {
            anchors.centerIn: grid
            visible: root.channels === 0
            text: root.session.status === "idle" ? "no song" : "this decoder shows no pattern"
            color: Theme.color.muted
        }
    }
}
