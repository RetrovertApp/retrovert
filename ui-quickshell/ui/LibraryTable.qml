import QtQuick
import QtQuick.Layouts
import "."

// Centre of the library: the search field and sort chips, then the module table.
Item {
    id: root

    // Column widths from the design's grid: 32 | 2fr | 1.1fr | 1.1fr | 44 | 30 | 44 | 48, 14 px gaps,
    // 10 px side padding. `fr` is what one flexible unit is worth in the row.
    readonly property int gap: 14
    readonly property real fr: (rows.width - 20 - (32 + 44 + 30 + 44 + 48) - 7 * gap) / 4.2
    readonly property var columns: [32, 2 * fr, 1.1 * fr, 1.1 * fr, 44, 30, 44, 48]

    ColumnLayout {
        anchors.fill: parent
        spacing: 0

        // Search and sort.
        Item {
            Layout.fillWidth: true
            implicitHeight: 62
            Rectangle { anchors.bottom: parent.bottom; width: parent.width; height: 1; color: Theme.color.line }
            RowLayout {
                anchors.fill: parent
                anchors.leftMargin: 22
                anchors.rightMargin: 22
                spacing: 14
                Rectangle {
                    Layout.fillWidth: true
                    implicitHeight: 34
                    color: Theme.color.surface
                    border.width: 1
                    border.color: Theme.color.line
                    Row {
                        anchors.fill: parent
                        anchors.leftMargin: 12
                        anchors.rightMargin: 12
                        spacing: 10
                        Mono { anchors.verticalCenter: parent.verticalCenter; text: "/"; font.pixelSize: Theme.font.body }
                        Mono {
                            anchors.verticalCenter: parent.verticalCenter
                            width: parent.width - 20
                            text: "title, composer, group, format:xm, ch:>8, year:199x"
                            color: Theme.color.muted
                            font.pixelSize: Theme.font.body
                        }
                    }
                }
                Row {
                    spacing: 6
                    Repeater {
                        model: ["Added", "Year", "Composer", "Plays"]
                        Rectangle {
                            required property string modelData
                            required property int index
                            width: chip.implicitWidth + 20
                            height: chip.implicitHeight + 12
                            color: index === 0 ? Theme.color.raised : "transparent"
                            Mono {
                                id: chip
                                anchors.centerIn: parent
                                text: parent.modelData
                                color: parent.index === 0 ? Theme.color.foreground : Theme.color.text
                            }
                        }
                    }
                }
            }
        }

        // The table.
        ColumnLayout {
            id: rows
            Layout.fillWidth: true
            Layout.fillHeight: true
            Layout.leftMargin: 22
            Layout.rightMargin: 22
            Layout.topMargin: 18
            spacing: 0

            RowLayout {
                Layout.fillWidth: true
                Layout.bottomMargin: 8
                Caption { text: "ALL FORMATS" }
                Item { Layout.fillWidth: true }
                Mono { text: "4,812 · 9d 06h"; color: Theme.color.muted }
            }

            // Header row.
            Item {
                Layout.fillWidth: true
                implicitHeight: 24
                Rectangle { anchors.bottom: parent.bottom; width: parent.width; height: 1; color: Theme.color.line }
                Row {
                    anchors.fill: parent
                    anchors.leftMargin: 10
                    anchors.rightMargin: 10
                    spacing: root.gap
                    Repeater {
                        model: ["#", "TITLE", "COMPOSER", "GROUP", "FMT", "CH", "YEAR", "TIME"]
                        Caption {
                            required property string modelData
                            required property int index
                            width: root.columns[index]
                            height: parent.height
                            verticalAlignment: Text.AlignVCenter
                            horizontalAlignment: index >= 5 ? Text.AlignRight : Text.AlignLeft
                            tracking: 0.1
                            text: modelData
                        }
                    }
                }
            }

            ListView {
                Layout.fillWidth: true
                Layout.fillHeight: true
                clip: true
                model: Mock.modules
                delegate: Rectangle {
                    id: row
                    required property var modelData
                    required property int index
                    readonly property bool playing: index === Mock.playingIndex
                    width: ListView.view.width
                    height: 33
                    color: playing ? Theme.color.raised : "transparent"
                    Row {
                        anchors.fill: parent
                        anchors.leftMargin: 10
                        anchors.rightMargin: 10
                        spacing: root.gap
                        Mono {
                            width: root.columns[0]; height: parent.height; verticalAlignment: Text.AlignVCenter
                            text: row.playing ? "▶" : String(row.index + 1).padStart(2, "0")
                            color: row.playing ? Theme.color.accent : Theme.color.muted
                        }
                        Text {
                            width: root.columns[1]; height: parent.height; verticalAlignment: Text.AlignVCenter
                            text: row.modelData.title
                            color: row.playing ? Theme.color.accent : Theme.color.foreground
                            font.family: Theme.fontFamily
                            font.pixelSize: Theme.font.subtitle
                            font.weight: row.playing ? Font.DemiBold : Font.Normal
                            elide: Text.ElideRight
                        }
                        Mono {
                            width: root.columns[2]; height: parent.height; verticalAlignment: Text.AlignVCenter
                            text: row.modelData.composer; font.pixelSize: Theme.font.subtitle
                        }
                        Mono {
                            width: root.columns[3]; height: parent.height; verticalAlignment: Text.AlignVCenter
                            text: row.modelData.group; color: Theme.color.muted; font.pixelSize: Theme.font.subtitle
                        }
                        FormatTag { anchors.verticalCenter: parent.verticalCenter; width: root.columns[4]; fmt: row.modelData.fmt }
                        Mono {
                            width: root.columns[5]; height: parent.height; verticalAlignment: Text.AlignVCenter
                            horizontalAlignment: Text.AlignRight; text: row.modelData.ch; color: Theme.color.muted
                        }
                        Mono {
                            width: root.columns[6]; height: parent.height; verticalAlignment: Text.AlignVCenter
                            horizontalAlignment: Text.AlignRight; text: row.modelData.year; color: Theme.color.muted
                        }
                        Mono {
                            width: root.columns[7]; height: parent.height; verticalAlignment: Text.AlignVCenter
                            horizontalAlignment: Text.AlignRight; text: row.modelData.dur; color: Theme.color.muted
                        }
                    }
                }
            }

        }
    }
}
