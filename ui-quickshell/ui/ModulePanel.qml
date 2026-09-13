import QtQuick
import QtQuick.Layouts
import Retrovert
import "."

// Right column of the library: the playing module, its samples, and the queue, read from the
// session's track and queue documents.
Rectangle {
    id: root
    required property Session session

    readonly property var track: session.track
    readonly property var queue: session.queue.items || []
    readonly property bool hasTrack: track.title !== undefined
    readonly property var names: (track.samples || []).length > 0 ? track.samples : (track.instruments || [])
    readonly property var facts: !hasTrack ? [] : [
        ["format", track.format + (track.plugin ? " · " + track.plugin : "")],
        ["channels", String(track.channels)],
        ["samples", (track.samples || []).length + " · " + (track.instruments || []).length + " instruments"],
        ["subsong", (track.subsong + 1) + " / " + (track.subsongs || []).length],
        ["file", track.file + " · " + Math.max(1, Math.round(track.size / 1024)) + " KB"]
    ]

    function statusMark() {
        switch (session.status) {
        case "playing": return "▶ playing"
        case "paused": return "❚❚ paused"
        case "finished": return "■ finished"
        case "error": return "✕ error"
        default: return "idle"
        }
    }

    color: Theme.color.surface
    clip: true
    Rectangle { width: 1; height: parent.height; color: Theme.color.line }

    ColumnLayout {
        anchors.fill: parent
        anchors.leftMargin: 1
        spacing: 0

        // Module.
        ColumnLayout {
            Layout.fillWidth: true
            Layout.margins: 18
            Layout.bottomMargin: 0
            spacing: 12
            RowLayout {
                Layout.fillWidth: true
                Caption { text: "MODULE" }
                Item { Layout.fillWidth: true }
                Mono {
                    text: root.statusMark()
                    color: root.session.status === "error" ? Theme.color.red
                         : root.session.status === "playing" ? Theme.color.accent : Theme.color.muted
                }
            }
            ColumnLayout {
                Layout.fillWidth: true
                spacing: 2
                Text {
                    Layout.fillWidth: true
                    text: root.hasTrack ? root.track.title : "nothing playing"
                    color: root.hasTrack ? Theme.color.foreground : Theme.color.muted
                    font.family: Theme.fontFamily
                    font.pixelSize: Theme.font.heading
                    font.weight: Font.DemiBold
                    elide: Text.ElideRight
                }
                Mono {
                    Layout.fillWidth: true
                    visible: root.hasTrack
                    text: (root.track.composer || "unknown composer") + (root.track.year > 0 ? " · " + root.track.year : "")
                    font.pixelSize: Theme.font.subtitle
                }
            }
            GridLayout {
                Layout.fillWidth: true
                columns: 2
                rowSpacing: 3
                columnSpacing: 14
                Repeater {
                    model: root.facts.length * 2
                    Mono {
                        required property int index
                        Layout.fillWidth: index % 2 === 1
                        text: root.facts[Math.floor(index / 2)][index % 2]
                        color: index % 2 === 0 ? Theme.color.muted : Theme.color.text
                    }
                }
            }
        }

        // Samples, faded out at the foot of a fixed-height window.
        ColumnLayout {
            Layout.fillWidth: true
            Layout.margins: 18
            Layout.topMargin: 16
            Layout.bottomMargin: 0
            spacing: 6
            visible: root.names.length > 0
            Caption { text: (root.track.samples || []).length > 0 ? "SAMPLES" : "INSTRUMENTS" }
            Item {
                Layout.fillWidth: true
                Layout.fillHeight: true
                Layout.maximumHeight: 132
                Layout.minimumHeight: 36
                clip: true
                Column {
                    width: parent.width
                    Repeater {
                        model: root.names
                        Mono {
                            required property string modelData
                            required property int index
                            width: parent.width
                            text: String(index + 1).padStart(2, "0") + " " + modelData
                            lineHeight: 1.55
                        }
                    }
                }
                Rectangle {
                    anchors.fill: parent
                    gradient: Gradient {
                        GradientStop { position: 0.7; color: "transparent" }
                        GradientStop { position: 1.0; color: Theme.color.surface }
                    }
                }
            }
        }

        // Queue.
        RowLayout {
            Layout.fillWidth: true
            Layout.margins: 18
            Layout.topMargin: 14
            Layout.bottomMargin: 6
            Caption { text: "QUEUE" }
            Item { Layout.fillWidth: true }
            Mono {
                text: root.queue.length === 0 ? "empty"
                    : root.queue.length + " · " + Theme.span(root.session.queue.total_ms || 0)
                color: Theme.color.muted
            }
        }
        Repeater {
            model: root.queue.slice(0, 7)
            Item {
                required property var modelData
                Layout.fillWidth: true
                Layout.leftMargin: 18
                Layout.rightMargin: 18
                implicitHeight: 26
                Row {
                    anchors.fill: parent
                    spacing: 10
                    Mono {
                        width: 36; height: parent.height; verticalAlignment: Text.AlignVCenter
                        text: modelData.format; color: Theme.color.muted; font.pixelSize: Theme.font.caption
                    }
                    Text {
                        width: parent.width - 36 - 40 - 20; height: parent.height; verticalAlignment: Text.AlignVCenter
                        textFormat: Text.StyledText
                        text: modelData.title + (modelData.composer ? " <font color=\"" + Theme.color.muted + "\">· " + modelData.composer + "</font>" : "")
                        color: Theme.color.foreground
                        font.family: Theme.fontFamily
                        font.pixelSize: Theme.font.body
                        elide: Text.ElideRight
                    }
                    Mono {
                        width: 40; height: parent.height; verticalAlignment: Text.AlignVCenter
                        horizontalAlignment: Text.AlignRight
                        text: modelData.duration_ms > 0 ? Theme.clock(modelData.duration_ms) : "—"; color: Theme.color.muted
                    }
                }
            }
        }
        Mono {
            Layout.leftMargin: 18
            visible: root.queue.length > 7
            text: "+ " + (root.queue.length - 7) + " more"
            color: Theme.color.muted
        }

        Item { Layout.fillHeight: true }

        Item {
            Layout.fillWidth: true
            implicitHeight: foot.implicitHeight + 24
            Rectangle { width: parent.width; height: 1; color: Theme.color.line }
            RowLayout {
                id: foot
                anchors.fill: parent
                anchors.margins: 18
                anchors.topMargin: 12
                anchors.bottomMargin: 12
                Mono {
                    Layout.fillWidth: true
                    text: "then: " + (root.session.loopMode === 2 ? "start the queue over"
                                    : root.session.loopMode === 1 ? "repeat the song" : "stop")
                    color: Theme.color.muted
                }
                Mono {
                    text: "clear"
                    color: root.queue.length > 0 ? Theme.color.text : Theme.color.muted
                    MouseArea {
                        anchors.fill: parent
                        anchors.margins: -6
                        cursorShape: Qt.PointingHandCursor
                        onClicked: root.session.clearQueue()
                    }
                }
            }
        }
    }
}
