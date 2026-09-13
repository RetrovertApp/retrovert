//! probe <plugin-dir> <song>: loads the decoders and tries a mount, printing what happened.
use retrovert_host::session::StreamFormat;
use retrovert_host::visualization::VisualizationConfig;
use retrovert_player::{PlaybackBackend, PlayerBackend};
use std::path::{Path, PathBuf};

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("debug")).init();
    let args: Vec<String> = std::env::args().collect();
    let dir = Path::new(&args[1]);
    let paths: Vec<PathBuf> = std::fs::read_dir(dir)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "so"))
        .collect();
    println!("plugins: {paths:?}");
    let mut backend = PlayerBackend::new(
        &paths,
        VisualizationConfig::default(),
        StreamFormat {
            sample_rate: 48_000,
            channels: 2,
        },
        4096,
    );
    println!("can handle hvl: {}", backend.can_handle_extension("hvl"));
    match backend.mount(Path::new(&args[2]), 0) {
        Ok(()) => {
            println!("mounted via {}", backend.plugin_name());
            let layout = backend.visualization_layout().cloned();
            println!(
                "layout: {:?}",
                layout.as_ref().map(|l| (
                    l.caps,
                    l.scope_channels.len(),
                    l.pattern_channels.len(),
                    l.columns.iter().map(|c| c.char_width).collect::<Vec<_>>()
                ))
            );
            let mut snap = layout.unwrap().new_snapshot().unwrap();
            for _ in 0..10 {
                let n = backend.render(1024).unwrap().len();
                backend.capture(&mut snap).unwrap();
                println!(
                    "rendered {n} samples; scope counts {:?}; vu {:?}; position {:?}; {} cells",
                    snap.scope_counts(),
                    snap.vu(),
                    snap.position,
                    snap.cells().len()
                );
            }
        }
        Err(e) => println!("mount failed: {e}"),
    }
}
