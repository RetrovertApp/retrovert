import QtQuick
import QtQuick.Layouts
import "."

// Right column of the library: the playing module, its samples, and the queue.
Rectangle {
    id: root
    property string title: "Phosphor Dreams"
    property string subtitle: "Tesseract / Pulsewidth · 1999"
    property string status: "playing"
    property var facts: [
        ["format", "IT · Impulse Tracker 2.14"], ["channels", "16"], ["patterns", "38 · 52 orders"],
        ["samples", "31 · 24 instruments"], ["subsong", "1 / 1"], ["file", "tsr-phosphor.it · 412 KB"]
    ]

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
                Mono { text: "▶ " + root.status; color: Theme.color.accent }
            }
            ColumnLayout {
                Layout.fillWidth: true
                spacing: 2
                Text {
                    Layout.fillWidth: true
                    text: root.title
                    color: Theme.color.foreground
                    font.family: Theme.fontFamily
                    font.pixelSize: Theme.font.heading
                    font.weight: Font.DemiBold
                    elide: Text.ElideRight
                }
                Mono { Layout.fillWidth: true; text: root.subtitle; font.pixelSize: Theme.font.subtitle }
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
            Caption { text: "SAMPLES" }
            Item {
                Layout.fillWidth: true
                Layout.fillHeight: true
                Layout.maximumHeight: 132
                Layout.minimumHeight: 36
                clip: true
                Column {
                    width: parent.width
                    Repeater {
                        model: Mock.samples
                        Mono { required property string modelData; width: parent.width; text: modelData; lineHeight: 1.55 }
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
            Mono { text: "7 · 25:18"; color: Theme.color.muted }
        }
        Repeater {
            model: Mock.queue
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
                        text: modelData.fmt; color: Theme.color.muted; font.pixelSize: Theme.font.caption
                    }
                    Text {
                        width: parent.width - 36 - 40 - 20; height: parent.height; verticalAlignment: Text.AlignVCenter
                        textFormat: Text.StyledText
                        text: modelData.title + " <font color=\"" + Theme.color.muted + "\">· " + modelData.composer + "</font>"
                        color: Theme.color.foreground
                        font.family: Theme.fontFamily
                        font.pixelSize: Theme.font.body
                        elide: Text.ElideRight
                    }
                    Mono {
                        width: 40; height: parent.height; verticalAlignment: Text.AlignVCenter
                        horizontalAlignment: Text.AlignRight; text: modelData.dur; color: Theme.color.muted
                    }
                }
            }
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
                Mono { Layout.fillWidth: true; text: "then: shuffle Party releases 2025"; color: Theme.color.muted }
                Mono { text: "clear" }
            }
        }
    }
}
