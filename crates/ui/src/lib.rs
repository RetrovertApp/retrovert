//! Flowi presentation for Retrovert visualization snapshots.

mod fasttracker_view;
mod general_view;
mod packed_faces;
mod pattern_display;
#[cfg(test)]
mod pixel_oracle;
mod pt_view;
mod retro_font;
mod scope_display;
mod sid_view;
mod tfmx_view;
mod v2m_view;
mod vgm_view;
mod view;
mod vu_display;

use flowi::Mount;
use retrovert_host::visualization::VizSnapshot;
use view::Views;

use crate::fasttracker_view::FastTrackerView;
use crate::general_view::GeneralView;
use crate::pt_view::PtView;
use crate::sid_view::SidView;
use crate::tfmx_view::TfmxView;
use crate::v2m_view::V2mView;
use crate::vgm_view::VgmView;

/// Host-owned values required to render one player frame.
pub struct RenderContext<'a> {
    /// Decoder name reported by the active playback plugin.
    pub decoder_name: &'a str,
    /// Mounted media extension without a leading dot.
    pub media_extension: &'a str,
    /// Coherent visualization data captured for this frame.
    pub snapshot: &'a VizSnapshot,
}

/// Stateful renderer collection for the seven player views.
pub struct PlayerUi {
    views: Views,
}

impl PlayerUi {
    /// Loads renderer resources from the consumer-provided asset root.
    pub fn new(assets: Option<&Mount>) -> Option<Self> {
        Some(Self {
            views: Views::new(
                PtView::new(assets)?,
                GeneralView::new(assets),
                FastTrackerView::new(assets)?,
                SidView::new(assets),
                TfmxView::new(assets),
                VgmView::new(assets),
                V2mView::new(assets),
            ),
        })
    }

    /// Selects and renders the view for one coherent host snapshot.
    pub fn render(&mut self, context: &RenderContext<'_>) {
        let kind = Views::select(
            context.decoder_name,
            context.media_extension,
            context.snapshot.layout.pattern_channels.len(),
        );
        if let Some(view) = self.views.get_mut(kind) {
            view.render(context.snapshot);
        }
    }
}

///////////////////////////////////////////////////////////////////////////////////////////////////////////////////////

#[cfg(test)]
mod integration_tests {
    use std::path::{Path, PathBuf};

    use retrovert_host::session::StreamFormat;
    use retrovert_host::visualization::{VisualizationConfig, VizSnapshot};
    use retrovert_player::{PlaybackBackend, PlayerBackend};

    use super::*;

    const SAMPLE_RATE: u32 = 48_000;
    const CHANNELS: u32 = 2;
    const BUFFER_FRAMES: u32 = 4_096;

    fn generated_protracker_module(build_dir: &Path, name: &str) -> PathBuf {
        let media_path = build_dir.join("test_data").join(name);
        let mut module = vec![0_u8; 1_084 + 1_024 + 4];
        module[..16].copy_from_slice(b"Replay PT oracle");
        module[42..44].copy_from_slice(&2_u16.to_be_bytes());
        module[45] = 64;
        module[48..50].copy_from_slice(&1_u16.to_be_bytes());
        module[950] = 1;
        module[1_080..1_084].copy_from_slice(b"M.K.");
        module[1_084..1_088].copy_from_slice(&[1, 172, 16, 0]);
        module[2_108..].copy_from_slice(&[0, 127, 0, 129]);
        if let Some(parent) = media_path.parent() {
            let created = std::fs::create_dir_all(parent);
            assert!(created.is_ok(), "{created:?}");
        }
        let written = std::fs::write(&media_path, module);
        assert!(written.is_ok(), "{written:?}");
        media_path
    }

    fn with_openmpt_snapshot(check: impl FnOnce(&VizSnapshot)) {
        let Some(build_dir) = std::env::var_os("BUILD_DIR") else {
            panic!("BUILD_DIR must identify the CMake build tree");
        };
        let build_dir = PathBuf::from(build_dir);
        let plugin_path =
            build_dir.join("plugins/music_player/playback_plugins/openmpt_playback.so");
        let media_path = generated_protracker_module(&build_dir, "generated_player_ui.mod");
        assert!(plugin_path.is_file(), "{}", plugin_path.display());

        let mut backend = PlayerBackend::new(
            &[plugin_path],
            VisualizationConfig::default(),
            StreamFormat {
                sample_rate: SAMPLE_RATE,
                channels: CHANNELS,
            },
            BUFFER_FRAMES,
        );
        let mounted = backend.mount(&media_path, 0);
        assert!(mounted.is_ok(), "{mounted:?}");
        let rendered = backend.render(2_048);
        assert!(rendered.is_ok(), "{rendered:?}");
        let Some(layout) = backend.visualization_layout().cloned() else {
            panic!("decoder did not publish its visualization layout");
        };
        let mut snapshot = layout
            .new_snapshot()
            .unwrap_or_else(|error| panic!("{error}"));
        let captured = backend.capture(&mut snapshot);
        assert!(captured.is_ok(), "{captured:?}");
        check(&snapshot);
    }

    #[test]
    #[ignore = "requires the CMake-built OpenMPT plugin and serial Flowi capture"]
    fn protracker_view_matches_blessed_png() {
        with_openmpt_snapshot(|snapshot| {
            let first_row_cells =
                snapshot.layout.columns.len() * snapshot.layout.pattern_channels.len();
            let first_row_text = snapshot
                .cells()
                .iter()
                .take(first_row_cells)
                .map(|cell| {
                    let length = cell
                        .text
                        .iter()
                        .position(|&byte| byte == 0)
                        .unwrap_or(cell.text.len());
                    format!(
                        "{}:{}",
                        cell.raw,
                        String::from_utf8_lossy(&cell.text[..length])
                    )
                })
                .collect::<Vec<_>>()
                .join("|");
            assert_eq!(
                first_row_text,
                include_str!("../tests/fixtures/music-player/pt_snapshot_text.tsv").trim_end()
            );

            let mut ui = None;
            let frame = crate::pixel_oracle::capture(1_920, 1_080, || {
                let Some(ui) = ui.get_or_insert_with(|| PlayerUi::new(None)) else {
                    panic!("player UI resources failed to initialize");
                };
                ui.render(&RenderContext {
                    decoder_name: "libopenmpt",
                    media_extension: "mod",
                    snapshot,
                });
            });
            crate::pixel_oracle::assert_png("pt_view", &frame);
        });
    }

    #[test]
    #[ignore = "requires the CMake-built OpenMPT plugin and serial Flowi capture"]
    fn fasttracker_view_matches_blessed_png() {
        with_openmpt_snapshot(|snapshot| {
            let mut view = None;
            let frame = crate::pixel_oracle::capture(1_920, 1_080, || {
                let Some(view) = view.get_or_insert_with(|| FastTrackerView::new(None)) else {
                    panic!("FastTracker resources failed to initialize");
                };
                view::View::render(view, snapshot);
            });
            crate::pixel_oracle::assert_png("fasttracker_view", &frame);
        });
    }

    #[test]
    #[ignore = "requires the CMake-built OpenMPT plugin and serial Flowi capture"]
    fn every_view_kind_renders_through_player_ui() {
        with_openmpt_snapshot(|snapshot| {
            for (decoder_name, media_extension) in [
                ("libopenmpt", "mod"),
                ("unknown", "mod"),
                ("libopenmpt", "xm"),
                ("sidplayfp", "sid"),
                ("tfmx", "mdat"),
                ("libvgm", "vgm"),
                ("v2m", "v2m"),
            ] {
                let mut ui = None;
                let frame = crate::pixel_oracle::capture(1_920, 1_080, || {
                    let Some(ui) = ui.get_or_insert_with(|| PlayerUi::new(None)) else {
                        panic!("player UI resources failed to initialize");
                    };
                    ui.render(&RenderContext {
                        decoder_name,
                        media_extension,
                        snapshot,
                    });
                });
                assert_eq!((frame.width(), frame.height()), (1_920, 1_080));
            }
        });
    }
}

///////////////////////////////////////////////////////////////////////////////////////////////////////////////////////

#[cfg(test)]
mod ui_tree_contract {
    use flowi::sha256_hex;

    const TREES: [(&str, &[u8], &str); 7] = [
        (
            "fasttracker",
            include_bytes!("../tests/fixtures/music-player/ui_tree/fasttracker.json"),
            "8f7647e8309ca5fe08d6b4a22e8d2ddd27a0fd0388e1d5e0b903d1a49ea4115e",
        ),
        (
            "general",
            include_bytes!("../tests/fixtures/music-player/ui_tree/general.json"),
            "1eeeed8b964ccc822a62f9d41cdb70ce107689b4c3bc110f0e2679516dc8a938",
        ),
        (
            "pt",
            include_bytes!("../tests/fixtures/music-player/ui_tree/pt.json"),
            "d51ac9fc1c1e4756126f890422b863c3ac0fca10107cd2b8e80d8b551a2db057",
        ),
        (
            "sid",
            include_bytes!("../tests/fixtures/music-player/ui_tree/sid.json"),
            "0c0ed352bde82ba22f213c0e4f224715119e7653e31b2f99b78657b9de1774c6",
        ),
        (
            "tfmx",
            include_bytes!("../tests/fixtures/music-player/ui_tree/tfmx.json"),
            "78bc48f58167647443f887c197522b0ea151351394d189e9bbb5de5793d13d7b",
        ),
        (
            "v2m",
            include_bytes!("../tests/fixtures/music-player/ui_tree/v2m.json"),
            "f352caeed6ff49f6114dc77ae62d67191b996a1636e94e3a2ab90c2d57106b88",
        ),
        (
            "vgm",
            include_bytes!("../tests/fixtures/music-player/ui_tree/vgm.json"),
            "983b252823d9c1dba916197d683fd67e130d9d021a00bc17146878d270c52f79",
        ),
    ];

    #[test]
    fn all_seven_ui_tree_fixtures_are_byte_exact() {
        for (name, fixture, expected) in TREES {
            assert_eq!(sha256_hex(fixture), expected, "{name}");
        }
    }
}
