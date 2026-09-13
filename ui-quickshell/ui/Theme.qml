pragma Singleton

import Quickshell
import Quickshell.Io
import QtQuick
import qs.Commons

// Feeds Omarchy's own Color and Style singletons from the current theme, the way the shell
// does for its bar, and reads the wider palette the design needs straight from the same
// colors.toml. Every role has the design's Tokyo Night value as its fallback, so a theme that
// lacks a key still draws. Font family and sizes are the desktop's, never the design's web fonts.
Singleton {
    id: root

    readonly property string stateDir: Quickshell.env("HOME") + "/.local/state/omarchy/current"

    // key -> "#rrggbb" for every colour line in colors.toml.
    property var palette: ({})

    function parse(raw) {
        var out = {}
        var lines = String(raw || "").split("\n")
        for (var i = 0; i < lines.length; i++) {
            var m = lines[i].match(/^\s*([A-Za-z0-9_-]+)\s*=\s*["']?(#[0-9A-Fa-f]{6})/)
            if (m) out[m[1]] = m[2]
        }
        return out
    }
    function pick(key, fallback) {
        var v = root.palette[key]
        return v ? v : fallback
    }

    readonly property QtObject color: QtObject {
        readonly property color background: root.pick("background", "#1a1b26")          // --bg
        readonly property color surface: root.pick("dark_background", "#16161e")        // --bg2
        readonly property color raised: root.pick("lighter_background", "#24283b")      // --bg3
        readonly property color foreground: root.pick("bright_foreground", "#c0caf5")   // --fg
        readonly property color text: root.pick("foreground", "#a9b1d6")                // --fg2
        readonly property color muted: root.pick("dark_foreground", "#565f89")          // --mute
        readonly property color line: root.pick("selection", "#292e42")                 // --line
        readonly property color accent: root.pick("accent", "#7aa2f7")
        readonly property color accent2: root.pick("bright_magenta", root.pick("magenta", "#bb9af7"))
        readonly property color green: root.pick("green", "#9ece6a")
        readonly property color red: root.pick("red", "#f7768e")
        readonly property color yellow: root.pick("yellow", "#e0af68")
    }

    readonly property string fontFamily: Style.font.family
    readonly property QtObject font: QtObject {
        readonly property int caption: Style.font.caption      // 10
        readonly property int small: Style.font.bodySmall      // 11
        readonly property int body: Style.font.body            // 12
        readonly property int subtitle: Style.font.subtitle    // 13
        readonly property int title: Style.font.title          // 14
        readonly property int heading: Style.font.heading      // 16
    }
    readonly property int cornerRadius: Style.cornerRadius

    // Colour for a format tag, the design's fmtColor table.
    function formatColor(fmt) {
        switch (fmt) {
        case "XM": return color.accent
        case "IT": return color.green
        case "SID": return color.yellow
        case "SPC": case "VGM": case "NSF": return color.red
        default: return color.accent2
        }
    }

    // blockLoading plus the onCompleted read applies the palette before the first frame.
    FileView {
        id: colorsFile
        path: root.stateDir + "/theme/colors.toml"
        blockLoading: true
        printErrors: false
        onLoaded: { Color.loadColors(text()); root.palette = root.parse(text()) }
        Component.onCompleted: { Color.loadColors(colorsFile.text()); root.palette = root.parse(colorsFile.text()) }
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
