pragma Singleton

import Quickshell
import Quickshell.Io
import QtQuick
import qs.Commons

// Feeds Omarchy's own Color and Style singletons from the current theme, the way the shell
// does for its bar, so every token below is the desktop's. Lifted from Flea's Theme.qml.
Singleton {
    id: root

    readonly property string stateDir: Quickshell.env("HOME") + "/.local/state/omarchy/current"

    readonly property QtObject color: QtObject {
        readonly property color background: Color.background
        readonly property color foreground: Color.foreground
        readonly property color accent: Color.accent
        readonly property color muted: Color.muted
    }
    readonly property string fontFamily: Style.font.family
    readonly property int cornerRadius: Style.cornerRadius

    // blockLoading plus the onCompleted read applies the palette before the first frame.
    FileView {
        id: colorsFile
        path: root.stateDir + "/theme/colors.toml"
        blockLoading: true
        printErrors: false
        onLoaded: Color.loadColors(text())
        Component.onCompleted: Color.loadColors(colorsFile.text())
    }
    FileView {
        id: shellFile
        path: root.stateDir + "/theme/shell.toml"
        blockLoading: true
        printErrors: false
        onLoaded: Color.loadShell(text())
        onLoadFailed: Color.loadShell("")
    }
    // theme.name is rewritten in place on a switch and so survives the directory swap.
    FileView {
        path: root.stateDir + "/theme.name"
        blockLoading: true
        watchChanges: true
        printErrors: false
        onFileChanged: { reload(); colorsFile.reload(); shellFile.reload(); Style.scheduleRefresh() }
    }
}
