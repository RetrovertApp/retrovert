import QtQuick
import QtQuick.Layouts
import Retrovert
import "."

// Left column of the library: wordmark, formats, collections, and the library root at the
// foot. Formats are the extensions the decoders claimed the rows by; collections are the
// directories the files sit in. Clicking a row filters the table, clicking it again clears.
Rectangle {
    id: root
    required property Session session
    required property var screen

    readonly property var rows: session.library.rows || []
    // [{label, count}] sorted by count, largest first, cut to what the column has room for;
    // a selected label past the cut is kept so the selection stays visible.
    readonly property int formatLimit: 9
    readonly property int collectionLimit: 6
    readonly property var formats: root.leading(root.group("format"), formatLimit, screen.formatFilter)
    readonly property var collections: root.leading(root.group("group"), collectionLimit, screen.collectionFilter)

    function leading(list, limit, keep) {
        if (list.length <= limit) return list
        var out = list.slice(0, limit)
        var kept = list.find(function (g) { return g.label === keep })
        if (kept && out.indexOf(kept) < 0) out[limit - 1] = kept
        out.push({ label: "+ " + (list.length - limit) + " more", count: "", more: true })
        return out
    }

    function group(key) {
        var counts = {}
        for (var i = 0; i < rows.length; i++) {
            var k = rows[i][key]
            if (!k) continue
            counts[k] = (counts[k] || 0) + 1
        }
        var out = Object.keys(counts).map(function (k) { return { label: k, count: counts[k] } })
        out.sort(function (a, b) { return b.count - a.count || a.label.localeCompare(b.label) })
        return out
    }

    color: Theme.color.surface
    Rectangle { anchors.right: parent.right; width: 1; height: parent.height; color: Theme.color.line }

    ColumnLayout {
        anchors.fill: parent
        anchors.topMargin: 18
        anchors.bottomMargin: 18
        anchors.rightMargin: 1
        spacing: 22

        Text {
            Layout.leftMargin: 18
            text: "RETROVERT"
            color: Theme.color.foreground
            font.family: Theme.fontFamily
            font.pixelSize: Theme.font.body
            font.weight: Font.DemiBold
            font.letterSpacing: Theme.font.body * 0.18
        }

        ColumnLayout {
            Layout.fillWidth: true
            spacing: 2
            Caption { Layout.leftMargin: 18; Layout.bottomMargin: 4; text: "FORMATS" }
            RailRow {
                label: "All formats"
                count: Theme.count(root.rows.length)
                selected: root.screen.formatFilter === ""
                onClicked: root.screen.formatFilter = ""
            }
            Repeater {
                model: root.formats
                RailRow {
                    required property var modelData
                    label: modelData.label
                    count: modelData.more ? "" : Theme.count(modelData.count)
                    selected: root.screen.formatFilter === modelData.label
                    enabled: !modelData.more
                    onClicked: root.screen.formatFilter = selected ? "" : modelData.label
                }
            }
        }

        ColumnLayout {
            Layout.fillWidth: true
            spacing: 2
            visible: root.collections.length > 0
            Caption { Layout.leftMargin: 18; Layout.bottomMargin: 4; text: "COLLECTIONS" }
            Repeater {
                model: root.collections
                RailRow {
                    required property var modelData
                    label: modelData.label
                    count: modelData.more ? "" : Theme.count(modelData.count)
                    selected: root.screen.collectionFilter === modelData.label
                    enabled: !modelData.more
                    onClicked: root.screen.collectionFilter = selected ? "" : modelData.label
                }
            }
        }

        Item { Layout.fillHeight: true }

        ColumnLayout {
            Layout.fillWidth: true
            Layout.leftMargin: 18
            Layout.rightMargin: 18
            spacing: 0
            Mono {
                Layout.fillWidth: true
                readonly property string libraryRoot: root.session.library.root || ""
                readonly property bool scanning: root.session.libraryScanned < root.session.libraryTotal
                text: libraryRoot.length === 0 ? "no library"
                    : Theme.tilde(libraryRoot) + " · " + (scanning
                        ? "scanning " + Theme.count(root.session.libraryScanned) + " / " + Theme.count(root.session.libraryTotal)
                        : Theme.count(root.rows.length) + " files")
                color: Theme.color.muted; lineHeight: 1.6; elide: Text.ElideNone; wrapMode: Text.WordWrap
            }
            Mono {
                Layout.fillWidth: true
                text: (root.session.library.plugins || []).join(" · ")
                color: Theme.color.muted; lineHeight: 1.6; elide: Text.ElideNone; wrapMode: Text.WordWrap
            }
        }
    }
}
