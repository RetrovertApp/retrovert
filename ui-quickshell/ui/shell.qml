//@ pragma AppId org.retrovert.player
//@ pragma ShellId retrovert
//@ pragma NativeTextRendering
//@ pragma CacheDir $BASE/retrovert

import Quickshell
import QtQuick
import Retrovert
import "."

// One window, one Rust session with the engine inside it, and the library screen over it,
// coloured by the Omarchy theme. RETROVERT_SONG names the file to play, RETROVERT_PLUGINS
// the directory of decoders and RETROVERT_LIBRARY the directory the library lists; the
// launcher sets all three.
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
        implicitWidth: 1416
        implicitHeight: 850
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
                var library = Quickshell.env("RETROVERT_LIBRARY")
                if (library) scanLibrary(library)
                var song = Quickshell.env("RETROVERT_SONG")
                if (song) open(song)
            }
        }

        Library {
            anchors.fill: parent
            session: player
        }
    }
}
