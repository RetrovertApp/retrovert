import QtQuick
import QtQuick.Layouts
import "."

// Left column of the library: wordmark, formats, collections, and the library root at the foot.
Rectangle {
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
            Repeater {
                model: Mock.formats
                RailRow { required property var modelData; required property int index
                          label: modelData.label; count: modelData.count; selected: index === 0 }
            }
        }

        ColumnLayout {
            Layout.fillWidth: true
            spacing: 2
            Caption { Layout.leftMargin: 18; Layout.bottomMargin: 4; text: "COLLECTIONS" }
            Repeater {
                model: Mock.collections
                RailRow { required property var modelData; label: modelData.label; count: modelData.count }
            }
        }

        Item { Layout.fillHeight: true }

        ColumnLayout {
            Layout.fillWidth: true
            Layout.leftMargin: 18
            Layout.rightMargin: 18
            spacing: 0
            Mono { Layout.fillWidth: true; text: "~/Music/modules · 4,812 files"; color: Theme.color.muted; lineHeight: 1.6; elide: Text.ElideNone; wrapMode: Text.WordWrap }
            Mono { Layout.fillWidth: true; text: "libopenmpt · libsidplayfp · game-music-emu"; color: Theme.color.muted; lineHeight: 1.6; elide: Text.ElideNone; wrapMode: Text.WordWrap }
        }
    }
}
