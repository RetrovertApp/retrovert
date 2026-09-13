//! `retrovert-gui [OPTIONS] [SONG]`: the command a user types.
//!
//! Quickshell owns `main()` and refuses arguments it does not know, so nothing typed on the
//! command line can reach the shell directly. This launcher is the ergonomic surface in
//! front of it: it resolves where the shell, the `Retrovert` QML module and the decoders
//! live, picks the renderer, and execs `qs` with all of that in variables the shell reads.
//! Those variables are a private channel between this binary and `ui/shell.qml`; no user
//! is expected to set them.

mod vulkan;

use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use clap::{Parser, ValueEnum};

/// Where the package installs each part; the dev tree is found relative to this crate.
const PACKAGED_UI: &str = "/usr/share/retrovert/ui";
const PACKAGED_QML: &str = "/usr/lib/retrovert/qml";
const PACKAGED_PLUGINS: &str = "/usr/lib/retrovert/plugins";
const DEV_ROOT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/..");
/// Where a full `cmake --build build` of the sibling `playback_plugins` checkout puts every
/// decoder, relative to this dev tree.
const DEV_PLUGINS: &str = "../../playback_plugins/build/plugins";

#[derive(Clone, Copy, Debug, ValueEnum)]
enum Renderer {
    Vulkan,
    Opengl,
}

#[derive(Parser, Debug)]
#[command(
    name = "retrovert-gui",
    about = "Retrovert, the Omarchy shell",
    version
)]
struct Args {
    /// The song to play on open.
    song: Option<PathBuf>,
    /// Directory of decoder plugins (.so). Defaults to the packaged set.
    #[arg(long, env = "RETROVERT_PLUGINS")]
    plugins: Option<PathBuf>,
    /// Directory the library lists, searched recursively. Defaults to ~/Music, else the
    /// song's own directory.
    #[arg(long, env = "RETROVERT_LIBRARY")]
    library: Option<PathBuf>,
    /// Directory holding shell.qml. Defaults to the packaged shell, then the dev tree.
    #[arg(long, env = "RETROVERT_UI")]
    ui: Option<PathBuf>,
    /// QML import path holding the Retrovert module. Defaults like --ui.
    #[arg(long, env = "RETROVERT_QML")]
    qml: Option<PathBuf>,
    /// Force a scene-graph backend instead of probing for Vulkan.
    #[arg(long, value_enum)]
    renderer: Option<Renderer>,
}

fn main() {
    let args = Args::parse();
    if !has_display() {
        eprintln!("retrovert-gui: there is no graphical session to open a window in");
        std::process::exit(2);
    }
    let Some(ui) = resolve(args.ui, PACKAGED_UI, "ui", |p| {
        p.join("shell.qml").is_file()
    }) else {
        eprintln!("retrovert-gui: no shell.qml found; set RETROVERT_UI or install the package");
        std::process::exit(2);
    };
    let Some(qml) = resolve(args.qml, PACKAGED_QML, "shim/build/qml", |p| {
        p.join("Retrovert").join("qmldir").is_file()
    }) else {
        eprintln!(
            "retrovert-gui: the Retrovert QML module is not built; run ui-quickshell/build.sh"
        );
        std::process::exit(2);
    };
    let plugins = resolve(args.plugins, PACKAGED_PLUGINS, DEV_PLUGINS, Path::is_dir)
        .unwrap_or_else(|| PathBuf::from(PACKAGED_PLUGINS));

    let mut cmd = Command::new("qs");
    cmd.arg("-p").arg(ui.join("shell.qml"));
    cmd.env("RETROVERT_PLUGINS", &plugins);
    let mut import_path = qml.into_os_string();
    if let Some(existing) = std::env::var_os("QML2_IMPORT_PATH").filter(|v| !v.is_empty()) {
        import_path.push(":");
        import_path.push(existing);
    }
    cmd.env("QML2_IMPORT_PATH", import_path);
    let song = args
        .song
        .map(|song| std::fs::canonicalize(&song).unwrap_or(song));
    match &song {
        Some(song) => {
            cmd.env("RETROVERT_SONG", song);
        }
        None => {
            cmd.env_remove("RETROVERT_SONG");
        }
    }
    match library_root(args.library, song.as_deref()) {
        Some(library) => {
            cmd.env("RETROVERT_LIBRARY", library);
        }
        None => {
            cmd.env_remove("RETROVERT_LIBRARY");
        }
    }
    choose_renderer(&mut cmd, args.renderer);
    // Qt 6.11's platform plugin registers the app with the portal's host registry, which
    // Quickshell has already done on the same connection, so Qt warns "Could not register app
    // ID" on every launch. Flea's shell prints the same line. Only that category is silenced,
    // and an operator's own rules are kept.
    if std::env::var_os("QT_LOGGING_RULES").is_none() {
        cmd.env("QT_LOGGING_RULES", "qt.qpa.services.warning=false");
    }

    // exec, so the shell replaces this process and no pid is orphaned. It only returns on failure.
    let error = cmd.exec();
    eprintln!("retrovert-gui: could not start qs: {error}");
    std::process::exit(1);
}

/// An explicit `--library`, else `~/Music` when it exists, else the directory the song lives
/// in. The music directory comes before the song's own because a song opened from a scratch
/// or download directory should not turn that directory into the library.
fn library_root(explicit: Option<PathBuf>, song: Option<&Path>) -> Option<PathBuf> {
    if let Some(root) = explicit {
        return Some(root);
    }
    let music = std::env::var_os("HOME").map(|home| PathBuf::from(home).join("Music"));
    if let Some(music) = music.filter(|m| m.is_dir()) {
        return Some(music);
    }
    song.and_then(Path::parent).map(Path::to_path_buf)
}

/// An explicit choice, then the packaged path, then the dev tree beside this crate.
fn resolve(
    explicit: Option<PathBuf>,
    packaged: &str,
    dev: &str,
    ok: impl Fn(&Path) -> bool,
) -> Option<PathBuf> {
    if let Some(p) = explicit {
        return ok(&p).then_some(p);
    }
    let packaged = PathBuf::from(packaged);
    if ok(&packaged) {
        return Some(packaged);
    }
    let dev = PathBuf::from(DEV_ROOT).join(dev);
    ok(&dev).then(|| std::fs::canonicalize(&dev).unwrap_or(dev))
}

/// The rule Flea's launcher follows: an operator's explicit `QSG_RHI_BACKEND` or `--renderer`
/// is never replaced; otherwise Vulkan when the probe says it can start, else OpenGL with the
/// reason said once, because a silent downgrade hides a 2.4x memory regression.
fn choose_renderer(cmd: &mut Command, forced: Option<Renderer>) {
    let explicit = std::env::var_os("QSG_RHI_BACKEND").is_some_and(|v| !v.is_empty());
    match forced {
        Some(Renderer::Vulkan) => cmd.env("QSG_RHI_BACKEND", "vulkan"),
        Some(Renderer::Opengl) => cmd.env("QSG_RHI_BACKEND", "opengl"),
        None if explicit => cmd.env_remove("RETROVERT_RENDERER_AUTOMATIC"),
        None => {
            match vulkan::usable() {
                Ok(()) => cmd
                    .env("QSG_RHI_BACKEND", "vulkan")
                    // Permits the shell its one relaunch on OpenGL if the scene graph still fails.
                    .env("RETROVERT_RENDERER_AUTOMATIC", "1"),
                Err(reason) => {
                    eprintln!("retrovert-gui: Vulkan is unusable, {reason}, so the shell starts on OpenGL");
                    cmd.env("QSG_RHI_BACKEND", "opengl")
                }
            }
        }
    };
}

fn has_display() -> bool {
    ["WAYLAND_DISPLAY", "DISPLAY"]
        .iter()
        .any(|name| std::env::var_os(name).is_some_and(|v| !v.is_empty()))
}
