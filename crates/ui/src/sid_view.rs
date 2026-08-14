use flowi::{col, fit, fixed, row, text_styled, Color, Font, FontId, Mount, Painter, TextConfig};
use retrovert_host::{
    ffi::playback::{RVColumnKind, RVPatternCell},
    visualization::VizSnapshot,
};

use crate::pattern_display::cell_text;
use crate::scope_display::ScopeDisplay;
use crate::view::View;
use crate::MONO_FONT_PATH;

const MAX_CHANNELS: usize = 9;
const WIDTH: f32 = 1_920.0;
const HEIGHT: f32 = 1_080.0;
const HEADER_WIDTH: f32 = WIDTH - 32.0;
const HEADER_HEIGHT: f32 = 140.0;
const HEADER_CONTENT_HEIGHT: f32 = HEADER_HEIGHT - 10.0;
const SCOPE_HEIGHT: f32 = 160.0;
const WAVEFORM_HEIGHT: f32 = 80.0;
const CONTAINER_X: f32 = 16.0;
const FONT_SIZE: u16 = 28;
const EYEBROW_FONT_SIZE: u16 = 20;
const TITLE_FONT_SIZE: u16 = 26;
const BYLINE_FONT_SIZE: u16 = 20;
const LINE_HEIGHT: f32 = FONT_SIZE as f32 + 2.0;

const BACKGROUND: Color = color(0x00, 0x00, 0x00, 0xff);
const SCOPE_BACKGROUND: Color = color(0x08, 0x08, 0x18, 0xff);
const HEADER_BACKGROUND: Color = color(0x40, 0x31, 0x8d, 0xff);
const ROW_CURRENT: Color = color(0x40, 0x31, 0x8d, 0x8c);
const ROW_HIGHLIGHT: Color = color(0x40, 0x31, 0x8d, 0x1f);
const TEXT_HEADER: Color = color(0xff, 0xff, 0xff, 0xff);
const TEXT_DIM: Color = color(0x50, 0x50, 0x50, 0xff);
const TEXT_HIGHLIGHT: Color = color(0xff, 0xff, 0xff, 0xff);
const SEPARATOR: Color = color(0x40, 0x31, 0x8d, 0x40);
const BORDER: Color = color(0x40, 0x31, 0x8d, 0xff);
const EFFECT: Color = color(0x67, 0xb6, 0xbd, 0xff);
const META_EYEBROW: Color = color(0x78, 0x69, 0xc4, 0xff);
const META_AUTHOR: Color = color(0x67, 0xb6, 0xbd, 0xff);
const META_SEPARATOR: Color = color(0x78, 0x69, 0xc4, 0xff);
const META_SUBTITLE: Color = color(0x9f, 0x9f, 0x9f, 0xff);
const VOICE_COLORS: [Color; 3] = [
    color(0x55, 0xa0, 0x49, 0xff),
    color(0xb8, 0x69, 0x62, 0xff),
    color(0x78, 0x69, 0xc4, 0xff),
];

const fn color(r: u8, g: u8, b: u8, a: u8) -> Color {
    Color { r, g, b, a }
}

pub struct SidView {
    font: FontId,
    scopes: ScopeDisplay,
}

impl SidView {
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
        col! { width: fixed!(HEADER_WIDTH), height: fixed!(HEADER_CONTENT_HEIGHT),
               padding: 12, gap: 4, color: HEADER_BACKGROUND {
            text_styled("NOW PLAYING", self.text(EYEBROW_FONT_SIZE, META_EYEBROW, 3));
            text_styled("", self.text(TITLE_FONT_SIZE, TEXT_HEADER, 1));
            row! { width: fit!(), height: fit!(), gap: 0 {
                text_styled("", self.text(BYLINE_FONT_SIZE, META_AUTHOR, 0));
                text_styled("  ·  ", self.text(BYLINE_FONT_SIZE, META_SEPARATOR, 0));
                text_styled("", self.text(BYLINE_FONT_SIZE, META_SUBTITLE, 0));
            }}
        }}
    }

    fn text(&self, size: u16, color: Color, letter_spacing: u16) -> TextConfig {
        TextConfig {
            font: Some(self.font),
            font_size: size,
            color,
            letter_spacing,
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
        let colors: [Color; MAX_CHANNELS] = std::array::from_fn(voice_color);
        let height = SCOPE_HEIGHT - 8.0;
        let width = WIDTH - CONTAINER_X * 2.0;
        self.scopes.draw_channels_oscilloscope(
            CONTAINER_X,
            HEADER_HEIGHT,
            width,
            height,
            &samples[..channels],
            &colors[..channels],
            SCOPE_BACKGROUND,
            SEPARATOR,
            4.0,
        );
        draw_border(CONTAINER_X, HEADER_HEIGHT, width, height, BORDER);
    }

    fn draw_pattern(&self, snapshot: &VizSnapshot) {
        let Some(position) = snapshot.position else {
            return;
        };
        let snapshot_channels = snapshot.layout.pattern_channels.len();
        let channels = snapshot_channels.min(MAX_CHANNELS);
        if channels == 0 || snapshot.layout.columns.is_empty() {
            return;
        }
        let available = HEIGHT - HEADER_HEIGHT - SCOPE_HEIGHT - WAVEFORM_HEIGHT;
        let visible_rows = (available / LINE_HEIGHT).floor().max(1.0) as u32;
        let half_visible = visible_rows / 2;
        let channel_width = WIDTH / channels as f32;
        let y = HEADER_HEIGHT + SCOPE_HEIGHT;
        let column_count = snapshot.layout.columns.len();

        for visible_row in 0..visible_rows {
            let row_y = y + visible_row as f32 * LINE_HEIGHT;
            let current = visible_row == half_visible;
            for channel in 0..channels {
                let current_row = snapshot
                    .channel_rows()
                    .get(channel)
                    .copied()
                    .unwrap_or(position.row);
                let row = i64::from(current_row) - i64::from(half_visible) + i64::from(visible_row);
                let channel_x = CONTAINER_X + channel as f32 * channel_width;
                if current {
                    Painter::rect(channel_x, row_y, channel_width, LINE_HEIGHT, ROW_CURRENT);
                } else if row >= 0 && row % 4 == 0 {
                    Painter::rect(channel_x, row_y, channel_width, LINE_HEIGHT, ROW_HIGHLIGHT);
                }

                let Some(row) = u32::try_from(row)
                    .ok()
                    .filter(|row| *row >= position.window_lo && *row < position.window_hi)
                else {
                    Painter::text(channel_x + 4.0, row_y, "--- .. ...", self.font, TEXT_DIM);
                    continue;
                };

                let row_offset = (row - position.window_lo) as usize;
                let mut cell_x = channel_x + 4.0;
                for (column, descriptor) in snapshot.layout.columns.iter().enumerate() {
                    let index = (row_offset * snapshot_channels + channel) * column_count + column;
                    let Some(cell) = snapshot.cells().get(index) else {
                        continue;
                    };
                    let text = std::str::from_utf8(cell_text(cell)).unwrap_or_default();
                    Painter::text(
                        cell_x,
                        row_y,
                        text,
                        self.font,
                        cell_color(descriptor.kind, cell, channel, current),
                    );
                    cell_x += f32::from(descriptor.char_width) * f32::from(FONT_SIZE) * 0.6;
                }
            }
        }
    }

    fn draw_separators(&self, snapshot: &VizSnapshot) {
        let channels = snapshot
            .layout
            .pattern_channels
            .len()
            .max(snapshot.layout.scope_channels.len())
            .min(MAX_CHANNELS);
        if channels < 2 {
            return;
        }
        let width = WIDTH / channels as f32;
        let bottom = HEIGHT - WAVEFORM_HEIGHT;
        for channel in 1..channels {
            Painter::rect(
                channel as f32 * width,
                HEADER_HEIGHT,
                1.0,
                bottom - HEADER_HEIGHT,
                BORDER,
            );
        }
    }
}

impl View for SidView {
    fn render(&mut self, snapshot: &VizSnapshot) {
        Painter::rect(0.0, 0.0, WIDTH, HEIGHT, BACKGROUND);
        self.draw_scopes(snapshot);
        self.draw_pattern(snapshot);
        self.draw_separators(snapshot);
        self.draw_header();
    }
}

fn voice_color(channel: usize) -> Color {
    VOICE_COLORS[channel % VOICE_COLORS.len()]
}

fn draw_border(x: f32, y: f32, width: f32, height: f32, color: Color) {
    Painter::rect(x, y, width, 1.0, color);
    Painter::rect(x, y + height - 1.0, width, 1.0, color);
    Painter::rect(x, y, 1.0, height, color);
    Painter::rect(x + width - 1.0, y, 1.0, height, color);
}

fn cell_color(kind: u32, cell: &RVPatternCell, channel: usize, current: bool) -> Color {
    if current {
        return TEXT_HIGHLIGHT;
    }
    if cell_text(cell)
        .iter()
        .copied()
        .all(|byte| matches!(byte, b'-' | b'.' | b'0' | b' '))
    {
        return TEXT_DIM;
    }
    if kind == RVColumnKind::Effect as u32 || kind == RVColumnKind::Param as u32 {
        EFFECT
    } else {
        voice_color(channel)
    }
}

///////////////////////////////////////////////////////////////////////////////////////////////////////////////////////

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn c_ui_tree_oracle_matches_the_ported_header_contract() {
        let oracle = include_str!("../tests/fixtures/music-player/ui_tree/sid.json");
        assert!(oracle.contains("\"elementCount\":8"));
        assert!(oracle.contains("\"width\":1888.00"));
        assert!(oracle.contains("\"height\":130.00"));
        assert!(oracle.contains("\"text\":\"NOW PLAYING\""));
        assert!(oracle.contains("\"text\":\"Oracle Module\""));
        assert!(oracle.contains("\"text\":\"Replay Test\""));
        assert!(oracle.contains("\"text\":\"Fixture Format\""));
        assert_eq!(rgba(HEADER_BACKGROUND), (64, 49, 141, 255));
        assert_eq!(rgba(META_EYEBROW), (120, 105, 196, 255));
        assert_eq!(rgba(META_AUTHOR), (103, 182, 189, 255));
    }

    #[test]
    fn plugin_rendered_cells_keep_sid_colors() {
        let note = cell(b"C-4");
        let empty = cell(b"---");
        assert_eq!(cell_text(&note), b"C-4");
        assert_eq!(
            rgba(cell_color(RVColumnKind::Note as u32, &note, 1, false)),
            rgba(VOICE_COLORS[1])
        );
        assert_eq!(
            rgba(cell_color(RVColumnKind::Effect as u32, &note, 1, false)),
            rgba(EFFECT)
        );
        assert_eq!(
            rgba(cell_color(RVColumnKind::Note as u32, &empty, 1, false)),
            rgba(TEXT_DIM)
        );
        assert_eq!(
            rgba(cell_color(RVColumnKind::Note as u32, &note, 1, true)),
            rgba(TEXT_HIGHLIGHT)
        );
    }

    fn cell(text: &[u8]) -> RVPatternCell {
        let mut cell = RVPatternCell {
            raw: 0,
            text: [0; 16],
        };
        cell.text[..text.len()].copy_from_slice(text);
        cell
    }

    fn rgba(color: Color) -> (u8, u8, u8, u8) {
        (color.r, color.g, color.b, color.a)
    }
}
