use flowi::{fit, fixed, grow, row, text_styled, Color, Font, FontId, Mount, Painter, TextConfig};
use retrovert_host::{
    ffi::playback::{RVColumnKind, RVPatternCell},
    visualization::VizSnapshot,
};

use crate::pattern_display::cell_text;
use crate::scope_display::ScopeDisplay;
use crate::view::View;

const MAX_CHANNELS: usize = 8;
const WIDTH: f32 = 1_920.0;
const HEIGHT: f32 = 1_080.0;
const HEADER_HEIGHT: f32 = 60.0;
const SCOPE_HEIGHT: f32 = 80.0;
const FONT_SIZE: u16 = 18;
const HEADER_FONT_SIZE: u16 = 28;
const LINE_HEIGHT: f32 = FONT_SIZE as f32 + 2.0;
const PATTERN_X: f32 = 8.0;

const BACKGROUND: Color = color(0x00, 0x11, 0x33, 0xff);
const HEADER_BACKGROUND: Color = color(0x22, 0x33, 0x55, 0xff);
const ROW_CURRENT: Color = color(0x33, 0x44, 0x66, 0xff);
const TEXT_NORMAL: Color = color(0x88, 0xcc, 0xff, 0xff);
const TEXT_DIM: Color = color(0x44, 0x66, 0x88, 0xff);
const TEXT_HIGHLIGHT: Color = color(0xff, 0xff, 0xff, 0xff);
const SEPARATOR: Color = color(0x00, 0x11, 0x22, 0xff);
const CHANNEL_COLORS: [Color; MAX_CHANNELS] = [
    color(0xc4, 0x70, 0x70, 0xff),
    color(0xb2, 0x80, 0x50, 0xff),
    color(0xa9, 0xb2, 0x50, 0xff),
    color(0x60, 0xb2, 0x50, 0xff),
    color(0x4f, 0xb2, 0x92, 0xff),
    color(0x4f, 0x71, 0xb2, 0xff),
    color(0x88, 0x50, 0xb2, 0xff),
    color(0xb2, 0x50, 0x91, 0xff),
];

const fn color(r: u8, g: u8, b: u8, a: u8) -> Color {
    Color { r, g, b, a }
}

pub struct TfmxView {
    font: FontId,
    scopes: ScopeDisplay,
}

impl TfmxView {
    pub fn new(assets: Option<&Mount>) -> Self {
        let font = assets.map_or_else(
            || Font::load("fonts/JetBrainsMono-Regular.ttf", FONT_SIZE),
            |mount| Font::load_mount(mount, "fonts/JetBrainsMono-Regular.ttf", FONT_SIZE),
        );
        Self {
            font,
            scopes: ScopeDisplay::default(),
        }
    }

    fn draw_header(&self) {
        row! { width: grow!(), height: fixed!(HEADER_HEIGHT), padding: 16, gap: 16,
               color: HEADER_BACKGROUND {
            row! { width: fit!(), height: fit!() {
                text_styled("TFMX:", self.header_text(TEXT_NORMAL));
            }}
            row! { width: grow!(), height: fit!() {
                text_styled("", self.header_text(TEXT_NORMAL));
            }}
            row! { width: fit!(), height: fit!() {
                text_styled("by", self.header_text(TEXT_DIM));
            }}
            row! { width: fit!(), height: fit!() {
                text_styled("", self.header_text(TEXT_NORMAL));
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
        let channels = snapshot.layout.scope_channels.len().min(MAX_CHANNELS);
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
        let channels = snapshot.layout.pattern_channels.len().min(MAX_CHANNELS);
        let columns = snapshot.layout.columns.len();
        if channels == 0 || columns == 0 {
            return;
        }

        let visible_rows = ((HEIGHT - HEADER_HEIGHT - SCOPE_HEIGHT - 24.0) / LINE_HEIGHT)
            .floor()
            .max(1.0) as u32;
        let half_visible = visible_rows / 2;
        let channel_width = WIDTH / channels as f32;
        let header_y = HEADER_HEIGHT + SCOPE_HEIGHT;
        self.draw_channel_headers(snapshot, channels, channel_width, header_y);
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
                let cell_x = PATTERN_X + channel as f32 * channel_width + 4.0;
                self.draw_cell_row(snapshot, channel, row, cell_x, row_y, current);
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

    fn draw_channel_headers(
        &self,
        snapshot: &VizSnapshot,
        channels: usize,
        channel_width: f32,
        y: f32,
    ) {
        for (channel, (descriptor, color)) in snapshot
            .layout
            .pattern_channels
            .iter()
            .zip(CHANNEL_COLORS)
            .take(channels)
            .enumerate()
        {
            let name = descriptor
                .name
                .split(|byte| *byte == 0)
                .next()
                .and_then(|bytes| std::str::from_utf8(bytes).ok())
                .unwrap_or_default();
            Painter::text(
                PATTERN_X + channel as f32 * channel_width + 4.0,
                y,
                name,
                self.font,
                color,
            );
        }
    }

    fn draw_cell_row(
        &self,
        snapshot: &VizSnapshot,
        channel: usize,
        row: i64,
        mut x: f32,
        y: f32,
        current: bool,
    ) {
        let Some(position) = snapshot.position else {
            return;
        };
        let Some(row) = u32::try_from(row)
            .ok()
            .filter(|row| *row >= position.window_lo && *row < position.window_hi)
        else {
            Painter::text(x, y, "--- -- -- ---", self.font, TEXT_DIM);
            return;
        };
        let snapshot_channels = snapshot.layout.pattern_channels.len();
        let columns = snapshot.layout.columns.len();
        let row_offset = (row - position.window_lo) as usize;
        let start = (row_offset * snapshot_channels + channel) * columns;
        let cells = snapshot
            .cells()
            .get(start..start + columns)
            .unwrap_or_default();
        if cells.is_empty() || cells.iter().all(|cell| cell_text(cell).is_empty()) {
            Painter::text(x, y, "--- -- -- ---", self.font, TEXT_DIM);
            return;
        }
        let route = note_route(cells, &snapshot.layout.columns, channel);
        for (cell, descriptor) in cells.iter().zip(snapshot.layout.columns.iter()) {
            let text = std::str::from_utf8(cell_text(cell)).unwrap_or_default();
            Painter::text(x, y, text, self.font, cell_color(cell, route, current));
            x += f32::from(descriptor.char_width) * FONT_SIZE as f32 * 0.6;
        }
    }
}

impl View for TfmxView {
    fn render(&mut self, snapshot: &VizSnapshot) {
        Painter::rect(0.0, 0.0, WIDTH, HEIGHT, BACKGROUND);
        self.draw_scopes(snapshot);
        self.draw_pattern(snapshot);
        self.draw_header();
    }
}

fn cell_color(cell: &RVPatternCell, route: usize, current: bool) -> Color {
    if cell.raw == 0
        || cell_text(cell)
            .iter()
            .all(|byte| matches!(byte, b'-' | b'.'))
    {
        TEXT_DIM
    } else if current {
        TEXT_HIGHLIGHT
    } else {
        CHANNEL_COLORS[route]
    }
}

fn note_route(
    cells: &[RVPatternCell],
    columns: &[retrovert_host::ffi::playback::RVColumnDesc],
    fallback: usize,
) -> usize {
    cells
        .iter()
        .zip(columns)
        .find(|(_, descriptor)| descriptor.kind == RVColumnKind::Note as u32)
        .map_or(fallback, |(cell, _)| {
            ((cell.raw >> 8) as usize) % MAX_CHANNELS
        })
}

///////////////////////////////////////////////////////////////////////////////////////////////////////////////////////

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn c_ui_tree_oracle_matches_the_header_contract() {
        let oracle = include_str!("../tests/fixtures/music-player/ui_tree/tfmx.json");
        assert!(oracle.contains("\"elementCount\":6"));
        assert!(oracle.contains("\"width\":1920.00"));
        assert!(oracle.contains("\"height\":60.00"));
        assert!(oracle.contains("\"text\":\"TFMX:\""));
        assert!(oracle.contains("\"text\":\"Oracle Module\""));
        assert!(oracle.contains("\"text\":\"Replay Test\""));
        assert_eq!(rgba(HEADER_BACKGROUND), (34, 51, 85, 255));
        assert_eq!(rgba(TEXT_NORMAL), (136, 204, 255, 255));
        assert_eq!(rgba(TEXT_DIM), (68, 102, 136, 255));
    }

    #[test]
    fn raw_note_routing_preserves_tfmx_colors() {
        let routed = cell(3 << 8 | 48, b"C-4");
        let empty = cell(0, b"..");
        let columns = [column(RVColumnKind::Instrument), column(RVColumnKind::Note)];
        let cells = [empty, routed];
        assert_eq!(note_route(&cells, &columns, 0), 3);
        assert_eq!(rgba(cell_color(&routed, 3, false)), rgba(CHANNEL_COLORS[3]));
        assert_eq!(rgba(cell_color(&routed, 3, true)), rgba(TEXT_HIGHLIGHT));
        assert_eq!(rgba(cell_color(&empty, 3, false)), rgba(TEXT_DIM));
        assert_eq!(rgba(cell_color(&empty, 3, true)), rgba(TEXT_DIM));
    }

    fn column(kind: RVColumnKind) -> retrovert_host::ffi::playback::RVColumnDesc {
        retrovert_host::ffi::playback::RVColumnDesc {
            label: [0; 16],
            char_width: 2,
            kind: kind as u32,
        }
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
