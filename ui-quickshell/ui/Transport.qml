import QtQuick
import QtQuick.Layouts
import "."

// The bar along the foot of the window: channel meter and title, transport, output and volume.
// The progress rule sits on its top edge.
Rectangle {
    id: root
    property string title: "Phosphor Dreams"
    property string subtitle: "Tesseract / Pulsewidth · IT 16ch"
    property int elapsedMs: 127000
    property int durationMs: 252000
    property bool playing: true
    property var vu: Mock.vu

    color: Theme.color.surface

    function clock(ms) {
        var s = Math.max(0, Math.floor(ms / 1000))
        return Math.floor(s / 60) + ":" + String(s % 60).padStart(2, "0")
    }

    Rectangle {
        width: parent.width
        height: 2
        color: Theme.color.line
        Rectangle {
            height: parent.height
            width: root.durationMs > 0 ? parent.width * Math.min(1, root.elapsedMs / root.durationMs) : 0
            color: Theme.color.accent
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
            Grid {
                anchors.fill: parent
                anchors.margins: 5
                columns: 4
                spacing: 2
                Repeater {
                    model: 16
                    Rectangle {
                        required property int index
                        width: (parent.width - 6) / 4
                        height: width
                        color: Theme.color.accent
                        opacity: root.vu[index]
                    }
                }
            }
        }
        ColumnLayout {
            Layout.fillWidth: true
            spacing: 3
            Text {
                Layout.fillWidth: true
                text: root.title
                color: Theme.color.foreground
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
            Mono { anchors.verticalCenter: parent.verticalCenter; text: "sub ‹ 1/1 ›"; color: Theme.color.muted }
            Mono { anchors.verticalCenter: parent.verticalCenter; text: "◀◀"; font.pixelSize: Theme.font.title }
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
            }
            Mono { anchors.verticalCenter: parent.verticalCenter; text: "▶▶"; font.pixelSize: Theme.font.title }
            Mono { anchors.verticalCenter: parent.verticalCenter; text: "loop ↻ 2×"; color: Theme.color.muted }
        }
        Text {
            Layout.alignment: Qt.AlignHCenter
            textFormat: Text.StyledText
            color: Theme.color.muted
            font.family: Theme.fontFamily
            font.pixelSize: Theme.font.small
            text: "<font color=\"" + Theme.color.text + "\">" + root.clock(root.elapsedMs) + "</font> / " + root.clock(root.durationMs)
                  + " &nbsp;·&nbsp; ord <font color=\"" + Theme.color.text + "\">27</font>/52 &nbsp;pat <font color=\""
                  + Theme.color.text + "\">0A</font> &nbsp;row <font color=\"" + Theme.color.text + "\">1C</font>"
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
            Mono { id: output; anchors.centerIn: parent; text: "48k · amiga filter off" }
        }
        Mono { anchors.verticalCenter: parent.verticalCenter; text: "vol"; color: Theme.color.muted }
        Rectangle {
            anchors.verticalCenter: parent.verticalCenter
            width: 100
            height: 2
            color: Theme.color.line
            Rectangle { width: parent.width * 0.4; height: parent.height; color: Theme.color.text }
        }
        Mono { anchors.verticalCenter: parent.verticalCenter; text: "40" }
    }
}
