//@ pragma AppId org.retrovert.player
//@ pragma ShellId retrovert
//@ pragma NativeTextRendering
//@ pragma CacheDir $BASE/retrovert

import Quickshell
import QtQuick
import Retrovert
import "."

// One window, one Rust session with the engine inside it, one scope per channel drawn by the
// scene-graph item in ../shim, coloured by the Omarchy theme. RETROVERT_SONG names the file
// to play and RETROVERT_PLUGINS the directory of decoders; run.sh sets both.
ShellRoot {
    // qs is a shell and outlives its windows by default; this is an application, so closing
    // the window ends the process. Stopping first lets the worker drop the decoder cleanly.
    Connections {
        target: Quickshell
        function onLastWindowClosed() { player.stop(); Qt.quit() }
    }

    FloatingWindow {
        id: window
        title: "Retrovert"
        implicitWidth: 800
        implicitHeight: 480
        color: Theme.color.background

        // Vulkan failing after launch raises this; only the launcher's own automatic choice may
        // retry, once, on OpenGL. An operator's explicit backend is never second-guessed.
        property bool relaunched: false
        Connections {
            target: window.contentItem ? window.contentItem.Window.window : null
            function onSceneGraphError(error, message) {
                console.warn("graphics backend failed (" + error + "): " + message)
                if (!window.relaunched && Quickshell.env("RETROVERT_RENDERER_AUTOMATIC") === "1") {
                    window.relaunched = true
                    Quickshell.execDetached(["/usr/bin/env", "QSG_RHI_BACKEND=opengl",
                                             "RETROVERT_RENDERER_AUTOMATIC=", "retrovert-gui",
                                             Quickshell.env("RETROVERT_SONG") || ""])
                }
                Qt.quit()
            }
        }

        // Not id: session, because inside Scope that name resolves to its own property and binds null.
        Session {
            id: player
            pluginDir: Quickshell.env("RETROVERT_PLUGINS") || ""
            running: true
            Component.onCompleted: {
                var song = Quickshell.env("RETROVERT_SONG")
                if (song) open(song)
            }
        }

        Column {
            anchors.fill: parent
            anchors.margins: 16
            spacing: 8

            Text {
                id: caption
                text: player.error.length > 0 ? player.error
                    : player.status === "idle" ? "no song, showing the demo waveform"
                    : (Quickshell.env("RETROVERT_SONG") || "").split("/").pop() + "  ·  " + player.plugin + "  ·  "
                      + player.status + "  ·  " + Math.floor(player.positionMs / 1000) + " s  ·  "
                      + player.scopeChannels + " channels"
                color: player.error.length > 0 ? Theme.color.accent : Theme.color.muted
                font.family: Theme.fontFamily
                elide: Text.ElideMiddle
                width: parent.width
            }

            Repeater {
                model: player.scopeChannels
                Rectangle {
                    required property int index
                    width: parent.width
                    height: (parent.height - caption.height - 8 * player.scopeChannels) / player.scopeChannels
                    color: "transparent"
                    border.color: Theme.color.muted
                    border.width: 1
                    radius: Theme.cornerRadius

                    Scope {
                        anchors.fill: parent
                        anchors.margins: 4
                        session: player
                        channel: parent.index
                        color: parent.index === 0 ? Theme.color.accent : Theme.color.foreground
                        lineWidth: 1
                    }
                }
            }
        }
    }
}
