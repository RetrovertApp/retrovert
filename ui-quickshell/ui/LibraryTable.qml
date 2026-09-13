import QtQuick
import QtQuick.Layouts
import Retrovert
import "."

// Centre of the library: the search field and sort chips, then the module table. The rows
// are the session's library filtered by the rail's selection and the search, in the chip's
// order; clicking one plays it and queues everything below it in that order.
Item {
    id: root
    required property Session session
    required property var screen

    // Column widths from the design's grid: 32 | 2fr | 1.1fr | 1.1fr | 44 | 30 | 44 | 48, 14 px gaps,
    // 10 px side padding. `fr` is what one flexible unit is worth in the row.
    readonly property int gap: 14
    readonly property real fr: (rows.width - 20 - (32 + 44 + 30 + 44 + 48) - 7 * gap) / 4.2
    readonly property var columns: [32, 2 * fr, 1.1 * fr, 1.1 * fr, 44, 30, 44, 48]

    readonly property var sortKeys: [
        { key: "added", label: "Added" }, { key: "year", label: "Year" },
        { key: "composer", label: "Composer" }, { key: "title", label: "Title" }
    ]
    readonly property var shown: root.select()
    readonly property int totalMs: shown.reduce(function (sum, r) { return sum + r.duration_ms }, 0)
    readonly property int playingRow: session.status === "idle" || session.track.row === undefined
                                    || session.track.row === null ? -1 : session.track.row

    function select() {
        var all = session.library.rows || []
        var fmt = screen.formatFilter
        var col = screen.collectionFilter
        var needle = screen.search.toLowerCase()
        var out = all.filter(function (r) {
            if (fmt && r.format !== fmt) return false
            if (col && r.group !== col) return false
            if (needle && (r.title + " " + r.composer + " " + r.group + " " + r.format).toLowerCase().indexOf(needle) < 0) return false
            return true
        })
        var key = screen.sortKey
        out.sort(function (a, b) {
            switch (key) {
            case "added": return b.added - a.added || a.title.localeCompare(b.title)
            case "year": return (b.year || 0) - (a.year || 0) || a.title.localeCompare(b.title)
            case "composer": return a.composer.localeCompare(b.composer) || a.title.localeCompare(b.title)
            default: return a.title.localeCompare(b.title)
            }
        })
        return out
    }

    function play(index) {
        session.playRows(shown.map(function (r) { return r.id }), index)
    }

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
                    border.color: search.activeFocus ? Theme.color.accent : Theme.color.line
                    Row {
                        anchors.fill: parent
                        anchors.leftMargin: 12
                        anchors.rightMargin: 12
                        spacing: 10
                        Mono { anchors.verticalCenter: parent.verticalCenter; text: "/"; font.pixelSize: Theme.font.body }
                        TextInput {
                            id: search
                            anchors.verticalCenter: parent.verticalCenter
                            width: parent.width - 20
                            color: Theme.color.foreground
                            font.family: Theme.fontFamily
                            font.pixelSize: Theme.font.body
                            selectByMouse: true
                            clip: true
                            onTextChanged: root.screen.search = text
                            Keys.onEscapePressed: { text = ""; focus = false }
                            Mono {
                                anchors.verticalCenter: parent.verticalCenter
                                visible: search.text.length === 0
                                text: "title, composer, collection, format"
                                color: Theme.color.muted
                                font.pixelSize: Theme.font.body
                            }
                        }
                    }
                    MouseArea {
                        anchors.fill: parent
                        cursorShape: Qt.IBeamCursor
                        onClicked: search.forceActiveFocus()
                    }
                }
                Row {
                    spacing: 6
                    Repeater {
                        model: root.sortKeys
                        Rectangle {
                            required property var modelData
                            readonly property bool active: root.screen.sortKey === modelData.key
                            width: chip.implicitWidth + 20
                            height: chip.implicitHeight + 12
                            color: active ? Theme.color.raised : "transparent"
                            Mono {
                                id: chip
                                anchors.centerIn: parent
                                text: parent.modelData.label
                                color: parent.active ? Theme.color.foreground : Theme.color.text
                            }
                            MouseArea {
                                anchors.fill: parent
                                cursorShape: Qt.PointingHandCursor
                                onClicked: root.screen.sortKey = parent.modelData.key
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
                Caption {
                    text: (root.screen.collectionFilter || root.screen.formatFilter || "ALL FORMATS").toUpperCase()
                        + (root.screen.collectionFilter && root.screen.formatFilter ? " · " + root.screen.formatFilter.toUpperCase() : "")
                }
                Item { Layout.fillWidth: true }
                Mono { text: Theme.count(root.shown.length) + " · " + Theme.span(root.totalMs); color: Theme.color.muted }
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
                        model: ["#", "TITLE", "COMPOSER", "COLLECTION", "FMT", "SUB", "YEAR", "TIME"]
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
                id: list
                Layout.fillWidth: true
                Layout.fillHeight: true
                clip: true
                model: root.shown
                // Idle and empty: say why the table is bare rather than show nothing.
                Mono {
                    anchors.centerIn: parent
                    visible: root.shown.length === 0
                    text: (root.session.library.root || "").length === 0 ? "no library directory"
                        : root.session.libraryScanned < root.session.libraryTotal ? "scanning…"
                        : root.screen.search.length > 0 ? "nothing matches" : "no songs here"
                    color: Theme.color.muted
                }
                delegate: Rectangle {
                    id: row
                    required property var modelData
                    required property int index
                    readonly property bool playing: modelData.id === root.playingRow
                    width: ListView.view.width
                    height: 33
                    color: playing ? Theme.color.raised : hover.containsMouse ? Theme.color.surface : "transparent"
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
                            text: row.modelData.composer || "—"; font.pixelSize: Theme.font.subtitle
                        }
                        Mono {
                            width: root.columns[3]; height: parent.height; verticalAlignment: Text.AlignVCenter
                            text: row.modelData.group || "—"; color: Theme.color.muted; font.pixelSize: Theme.font.subtitle
                        }
                        FormatTag { anchors.verticalCenter: parent.verticalCenter; width: root.columns[4]; fmt: row.modelData.format }
                        Mono {
                            width: root.columns[5]; height: parent.height; verticalAlignment: Text.AlignVCenter
                            horizontalAlignment: Text.AlignRight
                            text: row.modelData.subsongs > 1 ? row.modelData.subsongs : ""; color: Theme.color.muted
                        }
                        Mono {
                            width: root.columns[6]; height: parent.height; verticalAlignment: Text.AlignVCenter
                            horizontalAlignment: Text.AlignRight
                            text: row.modelData.year > 0 ? row.modelData.year : "—"; color: Theme.color.muted
                        }
                        Mono {
                            width: root.columns[7]; height: parent.height; verticalAlignment: Text.AlignVCenter
                            horizontalAlignment: Text.AlignRight
                            text: row.modelData.duration_ms > 0 ? Theme.clock(row.modelData.duration_ms) : "—"; color: Theme.color.muted
                        }
                    }
                    MouseArea {
                        id: hover
                        anchors.fill: parent
                        hoverEnabled: true
                        cursorShape: Qt.PointingHandCursor
                        onClicked: root.play(row.index)
                    }
                }
            }

        }
    }
}
