//! scopehold <plugin-dir> <song> [captures] [frames-per-capture]: renders like the worker
//! does and prints, per capture and channel, whether the scope picture repeated.
//! `=` unchanged from the previous capture, `0` all silence, `~` changed.
use retrovert_host::session::StreamFormat;
use retrovert_host::visualization::VisualizationConfig;
use retrovert_player::{PlaybackBackend, PlayerBackend};
use std::path::{Path, PathBuf};

fn hash(s: &[f32]) -> u64 {
    s.iter().fold(0xcbf2_9ce4_8422_2325_u64, |h, v| {
        (h ^ u64::from(v.to_bits())).wrapping_mul(0x0100_0000_01b3)
    })
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let dir = Path::new(&args[1]);
    let captures: usize = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(600);
    let chunk: u32 = args.get(4).and_then(|s| s.parse().ok()).unwrap_or(800);
    let paths: Vec<PathBuf> = std::fs::read_dir(dir)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "so"))
        .collect();
    let mut backend = PlayerBackend::new(
        &paths,
        VisualizationConfig::default(),
        StreamFormat {
            sample_rate: 48_000,
            channels: 2,
        },
        4096,
    );
    backend.mount(Path::new(&args[2]), 0).unwrap();
    println!("mounted via {}", backend.plugin_name());
    let layout = backend.visualization_layout().cloned().unwrap();
    let nch = layout.scope_channels.len();
    let mut snap = layout.new_snapshot().unwrap();
    let mut prev = vec![0_u64; nch];
    let mut run = vec![0_usize; nch];
    let mut worst = vec![0_usize; nch];
    let mut holds = vec![0_usize; nch];
    for i in 0..captures {
        backend.render(chunk).unwrap();
        backend.capture(&mut snap).unwrap();
        let mut line = String::new();
        for c in 0..nch {
            let s = snap.scope(c).unwrap_or(&[]);
            let h = hash(s);
            let silent = s.iter().all(|v| *v == 0.0);
            let ch = if silent {
                '0'
            } else if h == prev[c] {
                '='
            } else {
                '~'
            };
            if ch == '=' {
                run[c] += 1;
                if run[c] == 1 {
                    holds[c] += 1;
                }
                worst[c] = worst[c].max(run[c]);
            } else {
                run[c] = 0;
            }
            prev[c] = h;
            line.push(ch);
        }
        println!("{i:4} {line}");
    }
    println!("holds (runs of repeated non-silent pictures) per channel: {holds:?}");
    println!("longest hold in captures per channel: {worst:?}");
}
