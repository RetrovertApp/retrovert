use flowi::{
    col, fit, fixed, grow, row, text_styled, Color, Font, FontId, Mount, Painter, TextConfig,
};
use retrovert_host::{
    ffi::playback::{RVColumnKind, RVPatternCell},
    visualization::VizSnapshot,
};

use crate::pattern_display::cell_text;
use crate::scope_display::ScopeDisplay;
use crate::view::View;
use crate::MONO_FONT_PATH;

const MAX_CHANNELS: usize = 10;
const WIDTH: f32 = 1_920.0;
const HEIGHT: f32 = 1_080.0;
const HEADER_HEIGHT: f32 = 100.0;
const HEADER_LAYOUT_HEIGHT: f32 = 80.0;
const SCOPE_HEIGHT: f32 = 80.0;
const CHANNEL_HEADER_HEIGHT: f32 = 24.0;
const FONT_SIZE: u16 = 18;
const HEADER_FONT_SIZE: u16 = 28;
const LINE_HEIGHT: f32 = FONT_SIZE as f32 + 2.0;
const PATTERN_X: f32 = 8.0;

const BACKGROUND: Color = color(0x00, 0x08, 0x20, 0xff);
const HEADER_BACKGROUND: Color = color(0x18, 0x28, 0x48, 0xff);
const ROW_CURRENT: Color = color(0x20, 0x38, 0x60, 0xff);
const ROW_HIGHLIGHT: Color = color(0x08, 0x18, 0x38, 0xff);
const TEXT_NORMAL: Color = color(0x80, 0xb0, 0xe0, 0xff);
const TEXT_DIM: Color = color(0x30, 0x50, 0x70, 0xff);
const TEXT_HIGHLIGHT: Color = color(0xff, 0xff, 0xff, 0xff);
const SEPARATOR: Color = color(0x00, 0x04, 0x10, 0xff);
const EFFECT: Color = color(0xd0, 0xa0, 0x40, 0xff);
const CHANNEL_COLORS: [Color; MAX_CHANNELS] = [
    color(0xd0, 0x90, 0x70, 0xff),
    color(0xc0, 0x80, 0x60, 0xff),
    color(0xb0, 0x70, 0x50, 0xff),
    color(0xe0, 0x90, 0x80, 0xff),
    color(0xd0, 0x80, 0x70, 0xff),
    color(0xc0, 0x70, 0x60, 0xff),
    color(0x70, 0xc0, 0x70, 0xff),
    color(0x60, 0xb0, 0x60, 0xff),
    color(0x50, 0xa0, 0x50, 0xff),
    color(0x60, 0xc0, 0xc0, 0xff),
];
const CHANNEL_NAMES: [&str; MAX_CHANNELS] = [
    "FM 1", "FM 2", "FM 3", "FM 4", "FM 5", "FM 6", "PSG1", "PSG2", "PSG3", "NOIS",
];

const fn color(r: u8, g: u8, b: u8, a: u8) -> Color {
    Color { r, g, b, a }
}

pub struct VgmView {
    font: FontId,
    scopes: ScopeDisplay,
}

impl VgmView {
    pub fn new(assets: Option<&Mount>) -> Self {
        let font = assets.map_or_else(
            || Font::load(MONO_FONT_PATH, FONT_SIZE),
            |mount| Font::load_mount(mount, MONO_FONT_PATH, FONT_SIZE),
        );
        Self {
            font,
            scopes: ScopeDisplay::default(),
        }
    }

    fn draw_header(&self) {
        row! { width: grow!(), height: fixed!(HEADER_LAYOUT_HEIGHT), color: HEADER_BACKGROUND {
            row! { width: fit!(), height: grow!(), gap: 4 {
                col! { width: fit!(), height: fit!() {
                    text_styled("Title:", self.header_text(TEXT_DIM));
                    text_styled("Game:", self.header_text(TEXT_DIM));
                    text_styled("Author:", self.header_text(TEXT_DIM));
                }}
                col! { width: fit!(), height: fit!() {
                    text_styled("", self.header_text(TEXT_NORMAL));
                    text_styled("", self.header_text(TEXT_NORMAL));
                    text_styled("", self.header_text(TEXT_NORMAL));
                }}
            }}
            row! { width: grow!(), height: fit!() {}}
            row! { width: fit!(), height: fit!() {
                text_styled("", self.header_text(TEXT_DIM));
            }}
        }}
    }

    fn header_text(&self, color: Color) -> TextConfig {
        TextConfig {
            font: Some(self.font),
            font_size: HEADER_FONT_SIZE,
            color,
            no_wrap: true,
            ..Default::default()
        }
    }

    fn draw_scopes(&mut self, snapshot: &VizSnapshot) {
        let channels = displayed_channel_count(snapshot.layout().scope_channels.len());
        if channels == 0 {
            return;
        }
        let samples: [&[f32]; MAX_CHANNELS] =
            std::array::from_fn(|channel| snapshot.scope(channel).unwrap_or_default());
        self.scopes.draw_channels_oscilloscope(
            PATTERN_X,
            HEADER_HEIGHT,
            WIDTH,
            SCOPE_HEIGHT - 8.0,
            &samples[..channels],
            &CHANNEL_COLORS[..channels],
            BACKGROUND,
            SEPARATOR,
            4.0,
        );
    }

    fn draw_pattern(&self, snapshot: &VizSnapshot) {
        let Some(position) = snapshot.position else {
            return;
        };
        let channels = displayed_channel_count(snapshot.layout().pattern_channels.len());
        let columns = snapshot.layout().columns.len();
        if channels == 0 || columns == 0 {
            return;
        }

        let available = HEIGHT - HEADER_HEIGHT - SCOPE_HEIGHT - CHANNEL_HEADER_HEIGHT;
        let visible_rows = (available / LINE_HEIGHT).floor().max(1.0) as u32;
        let half_visible = visible_rows / 2;
        let channel_width = WIDTH / channels as f32;
        let header_y = HEADER_HEIGHT + SCOPE_HEIGHT;
        self.draw_channel_headers(channels, channel_width, header_y);
        let pattern_y = header_y + LINE_HEIGHT + 4.0;
        Painter::rect(
            PATTERN_X,
            pattern_y + half_visible as f32 * LINE_HEIGHT,
            WIDTH,
            LINE_HEIGHT,
            ROW_CURRENT,
        );

        for visible_row in 0..visible_rows {
            let row_y = pattern_y + visible_row as f32 * LINE_HEIGHT;
            let current = visible_row == half_visible;
            for channel in 0..channels {
                let channel_row = snapshot
                    .channel_rows()
                    .get(channel)
                    .copied()
                    .unwrap_or(position.row);
                let row = i64::from(channel_row) - i64::from(half_visible) + i64::from(visible_row);
                let valid_row = u32::try_from(row)
                    .ok()
                    .filter(|row| *row >= position.window_lo && *row < position.window_hi);
                if !current && valid_row.is_some_and(|row| row % 4 == 0) {
                    Painter::rect(
                        PATTERN_X + channel as f32 * channel_width,
                        row_y,
                        channel_width,
                        LINE_HEIGHT,
                        ROW_HIGHLIGHT,
                    );
                }
                let cell_x = PATTERN_X + channel as f32 * channel_width + 4.0;
                self.draw_cell_row(snapshot, channel, valid_row, cell_x, row_y, current);
            }
            for channel in 1..channels {
                Painter::rect(
                    PATTERN_X + channel as f32 * channel_width - 1.0,
                    row_y,
                    1.0,
                    LINE_HEIGHT,
                    SEPARATOR,
                );
            }
        }
    }

    fn draw_channel_headers(&self, channels: usize, channel_width: f32, y: f32) {
        for channel in 0..channels {
            Painter::text(
                PATTERN_X + channel as f32 * channel_width + 4.0,
                y,
                CHANNEL_NAMES[channel],
                self.font,
                CHANNEL_COLORS[channel],
            );
        }
    }

    fn draw_cell_row(
        &self,
        snapshot: &VizSnapshot,
        channel: usize,
        row: Option<u32>,
        mut x: f32,
        y: f32,
        current: bool,
    ) {
        let Some(position) = snapshot.position else {
            return;
        };
        let Some(row) = row else {
            Painter::text(x, y, "--- -- -- ---", self.font, TEXT_DIM);
            return;
        };
        let snapshot_channels = snapshot.layout().pattern_channels.len();
        let columns = snapshot.layout().columns.len();
        let row_offset = (row - position.window_lo) as usize;
        let start = (row_offset * snapshot_channels + channel) * columns;
        let cells = snapshot
            .cells()
            .get(start..start + columns)
            .unwrap_or_default();
        if cells.is_empty()
            || cells.iter().all(|cell| {
                cell_text(cell)
                    .iter()
                    .all(|byte| matches!(byte, b'-' | b'.'))
            })
        {
            Painter::text(x, y, "--- -- -- ---", self.font, TEXT_DIM);
            return;
        }

        for (cell, descriptor) in cells.iter().zip(snapshot.layout().columns.iter()) {
            let text = std::str::from_utf8(cell_text(cell)).unwrap_or_default();
            Painter::text(
                x,
                y,
                text,
                self.font,
                cell_color(cell, descriptor.kind, channel, current),
            );
            x += f32::from(descriptor.char_width) * FONT_SIZE as f32 * 0.6;
        }
    }
}

impl View for VgmView {
    fn render(&mut self, snapshot: &VizSnapshot) {
        Painter::rect(0.0, 0.0, WIDTH, HEIGHT, BACKGROUND);
        self.draw_scopes(snapshot);
        self.draw_header();
        self.draw_pattern(snapshot);
    }
}

fn displayed_channel_count(snapshot_channels: usize) -> usize {
    snapshot_channels.min(MAX_CHANNELS)
}

fn cell_color(cell: &RVPatternCell, kind: u32, channel: usize, current: bool) -> Color {
    if cell_text(cell)
        .iter()
        .all(|byte| matches!(byte, b'-' | b'.'))
    {
        TEXT_DIM
    } else if kind == RVColumnKind::Effect as u32 || kind == RVColumnKind::Param as u32 {
        EFFECT
    } else if cell.raw == 0 {
        TEXT_DIM
    } else if current {
        TEXT_HIGHLIGHT
    } else {
        CHANNEL_COLORS[channel]
    }
}

///////////////////////////////////////////////////////////////////////////////////////////////////////////////////////

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn c_ui_tree_oracle_matches_the_header_contract() {
        let oracle = include_str!("../tests/fixtures/music-player/ui_tree/vgm.json");
        assert!(oracle.contains("\"elementCount\":9"));
        assert!(oracle.contains("\"width\":1920.00"));
        assert!(oracle.contains("\"height\":80.00"));
        assert!(oracle.contains("\"text\":\"Title:\""));
        assert!(oracle.contains("\"text\":\"Game:\""));
        assert!(oracle.contains("\"text\":\"Author:\""));
        assert!(oracle.contains("\"text\":\"Oracle Module\""));
        assert!(oracle.contains("\"text\":\"Fixture Format\""));
        assert!(oracle.contains("\"text\":\"Replay Test\""));
        assert!(oracle.contains("\"text\":\"Reference System\""));
        assert_eq!(rgba(HEADER_BACKGROUND), (24, 40, 72, 255));
        assert_eq!(rgba(TEXT_NORMAL), (128, 176, 224, 255));
        assert_eq!(rgba(TEXT_DIM), (48, 80, 112, 255));
    }

    #[test]
    fn snapshot_cells_keep_vgm_palette_roles() {
        let note = cell(48, b"C-3");
        let empty = cell(0, b"..");
        let effect = cell(1, b"V");
        let parameter = cell(0, b"00");

        assert_eq!(
            rgba(cell_color(&note, RVColumnKind::Note as u32, 6, false)),
            rgba(CHANNEL_COLORS[6])
        );
        assert_eq!(
            rgba(cell_color(&note, RVColumnKind::Note as u32, 6, true)),
            rgba(TEXT_HIGHLIGHT)
        );
        assert_eq!(
            rgba(cell_color(&empty, RVColumnKind::Volume as u32, 6, true)),
            rgba(TEXT_DIM)
        );
        assert_eq!(
            rgba(cell_color(&effect, RVColumnKind::Effect as u32, 6, true)),
            rgba(EFFECT)
        );
        assert_eq!(
            rgba(cell_color(&parameter, RVColumnKind::Param as u32, 6, false)),
            rgba(EFFECT)
        );
    }

    #[test]
    fn displayed_channels_are_capped() {
        assert_eq!(displayed_channel_count(0), 0);
        assert_eq!(displayed_channel_count(MAX_CHANNELS + 1), MAX_CHANNELS);
    }

    fn cell(raw: u32, text: &[u8]) -> RVPatternCell {
        let mut cell = RVPatternCell { raw, text: [0; 16] };
        cell.text[..text.len()].copy_from_slice(text);
        cell
    }

    fn rgba(color: Color) -> (u8, u8, u8, u8) {
        (color.r, color.g, color.b, color.a)
    }
}
