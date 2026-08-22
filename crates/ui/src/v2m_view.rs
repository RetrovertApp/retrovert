use flowi::{
    fit, fixed, row, text_styled, BorderWidthConfig, Color, CornerRadius, Font, FontId, Layout,
    LayoutAlignmentX, LayoutAlignmentY, LayoutBorder, LayoutChildAlignment, LayoutClip,
    LayoutConfig, LayoutDirection, LayoutEndGuard, LayoutPadding, LayoutPlaced, LayoutSizing,
    Mount, Painter, StrokeStyle, TextConfig, Vec2,
};
use retrovert_host::{
    ffi::playback::{RVColumnKind, RVPatternCell},
    visualization::VizSnapshot,
};

use crate::pattern_display::cell_text;
use crate::scope_display::ScopeDisplay;
use crate::view::View;
use crate::MONO_FONT_PATH;

const MAX_CHANNELS: usize = 16;
const WIDTH: f32 = 1_920.0;
const HEIGHT: f32 = 1_080.0;
const HEADER_HEIGHT: f32 = 140.0;
const HEADER_CONTENT_HEIGHT: f32 = 130.0;
const SCOPE_HEIGHT: f32 = 160.0;
const CONTAINER_X: f32 = 16.0;
const FONT_SIZE: u16 = 18;
const EYEBROW_FONT_SIZE: u16 = 20;
const TITLE_FONT_SIZE: u16 = 26;
const BYLINE_FONT_SIZE: u16 = 20;
const LINE_HEIGHT: f32 = FONT_SIZE as f32 + 2.0;

const SCOPE_BACKGROUND: Color = color(0x08, 0x08, 0x18, 0xff);
const HEADER_BACKGROUND: Color = color(0x2a, 0x2a, 0x5a, 0xff);
const ROW_CURRENT: Color = color(0x3a, 0x3a, 0x7a, 0x8c);
const ROW_HIGHLIGHT: Color = color(0x2a, 0x2a, 0x5a, 0x1f);
const TEXT_DIM: Color = color(0x50, 0x50, 0x50, 0xff);
const TEXT_HIGHLIGHT: Color = color(0xff, 0xff, 0xff, 0xff);
const SEPARATOR: Color = color(0x3a, 0x3a, 0x7a, 0x40);
const BORDER: Color = color(0x3a, 0x3a, 0x7a, 0xff);
const NOTE: Color = color(0x80, 0xc0, 0xff, 0xff);
const INSTRUMENT: Color = color(0xff, 0xc0, 0x80, 0xff);
const EFFECT: Color = color(0x67, 0xb6, 0xbd, 0xff);
const META_EYEBROW: Color = color(0x60, 0x60, 0xb0, 0xff);
const META_AUTHOR: Color = color(0x67, 0xb6, 0xbd, 0xff);
const META_SEPARATOR: Color = color(0x60, 0x60, 0xb0, 0xff);
const META_SUBTITLE: Color = color(0x9f, 0x9f, 0x9f, 0xff);
const CHANNEL_COLORS: [Color; 8] = [
    color(0x80, 0xc0, 0xff, 0xff),
    color(0xff, 0x80, 0x80, 0xff),
    color(0x80, 0xff, 0x80, 0xff),
    color(0xff, 0xc0, 0x80, 0xff),
    color(0xc0, 0x80, 0xff, 0xff),
    color(0xff, 0xff, 0x80, 0xff),
    color(0x80, 0xff, 0xc0, 0xff),
    color(0xff, 0x80, 0xc0, 0xff),
];

const fn color(r: u8, g: u8, b: u8, a: u8) -> Color {
    Color { r, g, b, a }
}

pub struct V2mView {
    font: FontId,
    scopes: ScopeDisplay,
}

impl V2mView {
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
        Layout::start_auto();
        Layout::configure(header_config());
        let _end = LayoutEndGuard::new();
        text_styled("NOW PLAYING", self.text(EYEBROW_FONT_SIZE, META_EYEBROW, 3));
        text_styled("", self.text(TITLE_FONT_SIZE, TEXT_HIGHLIGHT, 1));
        row! { width: fit!(), height: fit!(), gap: 0 {
            text_styled("", self.text(BYLINE_FONT_SIZE, META_AUTHOR, 0));
            text_styled("  ·  ", self.text(BYLINE_FONT_SIZE, META_SEPARATOR, 0));
            text_styled("", self.text(BYLINE_FONT_SIZE, META_SUBTITLE, 0));
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
        let channels = displayed_channel_count(snapshot.layout().scope_channels.len());
        if channels == 0 {
            return;
        }
        let samples: [&[f32]; MAX_CHANNELS] =
            std::array::from_fn(|channel| snapshot.scope(channel).unwrap_or_default());
        let colors: [Color; MAX_CHANNELS] =
            std::array::from_fn(|channel| CHANNEL_COLORS[channel % CHANNEL_COLORS.len()]);
        let width = WIDTH - CONTAINER_X * 2.0;
        self.scopes.draw_channels_oscilloscope(
            CONTAINER_X,
            HEADER_HEIGHT,
            width,
            SCOPE_HEIGHT,
            &samples[..channels],
            &colors[..channels],
            SCOPE_BACKGROUND,
            SEPARATOR,
            4.0,
        );
        Painter::rect(CONTAINER_X, HEADER_HEIGHT, width, 1.0, BORDER);
        Painter::rect(
            CONTAINER_X,
            HEADER_HEIGHT + SCOPE_HEIGHT - 1.0,
            width,
            1.0,
            BORDER,
        );
        Painter::rect(CONTAINER_X, HEADER_HEIGHT, 1.0, SCOPE_HEIGHT, BORDER);
        Painter::rect(
            CONTAINER_X + width - 1.0,
            HEADER_HEIGHT,
            1.0,
            SCOPE_HEIGHT,
            BORDER,
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

        let pattern_y = HEADER_HEIGHT + SCOPE_HEIGHT;
        let pattern_height = HEIGHT - pattern_y;
        let visible_rows = (pattern_height / LINE_HEIGHT).floor() as u32 + 4;
        let half_visible = visible_rows / 2;
        let width = WIDTH - CONTAINER_X * 2.0;
        let channel_width = width / channels as f32;
        Painter::rect(
            CONTAINER_X,
            pattern_y + half_visible as f32 * LINE_HEIGHT,
            width,
            LINE_HEIGHT,
            ROW_CURRENT,
        );

        for visible_row in 0..visible_rows {
            let y = pattern_y + visible_row as f32 * LINE_HEIGHT;
            let current = visible_row == half_visible;
            let row = i64::from(position.row) - i64::from(half_visible) + i64::from(visible_row);
            let valid_row = u32::try_from(row)
                .ok()
                .filter(|row| *row >= position.window_lo && *row < position.window_hi);
            if !current && valid_row.is_some_and(|row| row % 4 == 0) {
                Painter::rect(CONTAINER_X, y, width, LINE_HEIGHT, ROW_HIGHLIGHT);
            }
            for channel in 0..channels {
                let x = CONTAINER_X + channel as f32 * channel_width + 4.0;
                self.draw_cell_row(snapshot, channel, valid_row, x, y, current);
            }
        }

        for channel in 1..channels {
            Painter::rect(
                CONTAINER_X + channel as f32 * channel_width,
                pattern_y,
                1.0,
                pattern_height,
                BORDER,
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
            Painter::text(x, y, "--- .. ...", self.font, TEXT_DIM);
            return;
        };
        let channels = snapshot.layout().pattern_channels.len();
        let columns = snapshot.layout().columns.len();
        let row_offset = (row - position.window_lo) as usize;
        let start = (row_offset * channels + channel) * columns;
        let cells = snapshot
            .cells()
            .get(start..start + columns)
            .unwrap_or_default();
        if cells.is_empty() {
            Painter::text(x, y, "--- .. ...", self.font, TEXT_DIM);
            return;
        }

        for (cell, descriptor) in cells.iter().zip(snapshot.layout().columns.iter()) {
            let text = std::str::from_utf8(cell_text(cell)).unwrap_or_default();
            Painter::text(
                x,
                y,
                text,
                self.font,
                cell_color(cell, descriptor.kind, current),
            );
            x += f32::from(descriptor.char_width) * FONT_SIZE as f32 * 0.6;
        }
    }
}

impl View for V2mView {
    fn render(&mut self, snapshot: &VizSnapshot) {
        self.draw_header();
        self.draw_scopes(snapshot);
        self.draw_pattern(snapshot);
    }
}

fn displayed_channel_count(snapshot_channels: usize) -> usize {
    snapshot_channels.min(MAX_CHANNELS)
}

fn header_config() -> LayoutConfig {
    LayoutConfig {
        sizing: LayoutSizing {
            width: fixed!(WIDTH - CONTAINER_X * 2.0),
            height: fixed!(HEADER_CONTENT_HEIGHT),
        },
        padding: LayoutPadding {
            left: 12,
            right: 16,
            top: 12,
            bottom: 16,
        },
        child_gap: 4,
        radius: CornerRadius::from(0.0),
        child_alignment: LayoutChildAlignment {
            x: LayoutAlignmentX::Left,
            y: LayoutAlignmentY::Top,
        },
        direction: LayoutDirection::TopToBottom,
        color: HEADER_BACKGROUND,
        clip: LayoutClip {
            horizontal: false,
            vertical: false,
            manual_scroll: false,
            scroll_pos: Vec2 { x: 0.0, y: 0.0 },
        },
        border: LayoutBorder {
            color: SEPARATOR,
            width: BorderWidthConfig {
                left: 0,
                right: 0,
                top: 0,
                bottom: 10,
                between_children: 0,
            },
            stroke: StrokeStyle {
                dash_len: 0.0,
                gap_len: 0.0,
            },
        },
        placed: LayoutPlaced {
            offset: Vec2 { x: 0.0, y: 0.0 },
            enabled: false,
        },
    }
}

fn cell_color(cell: &RVPatternCell, kind: u32, current: bool) -> Color {
    if cell.raw == 0
        || cell_text(cell)
            .iter()
            .all(|byte| matches!(byte, b'-' | b'.'))
    {
        TEXT_DIM
    } else if current {
        TEXT_HIGHLIGHT
    } else if kind == RVColumnKind::Note as u32 {
        NOTE
    } else if kind == RVColumnKind::Instrument as u32 {
        INSTRUMENT
    } else {
        EFFECT
    }
}

///////////////////////////////////////////////////////////////////////////////////////////////////////////////////////

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn c_ui_tree_oracle_matches_the_header_contract() {
        let oracle = include_str!("../tests/fixtures/music-player/ui_tree/v2m.json");
        assert!(oracle.contains("\"elementCount\":8"));
        assert!(oracle.contains("\"width\":1888.00"));
        assert!(oracle.contains("\"height\":130.00"));
        assert!(oracle.contains("\"text\":\"NOW PLAYING\""));
        assert!(oracle.contains("\"text\":\"Oracle Module\""));
        assert!(oracle.contains("\"text\":\"Replay Test\""));
        assert!(oracle.contains("\"text\":\"Fixture Format\""));
        assert_eq!(rgba(HEADER_BACKGROUND), (42, 42, 90, 255));
        assert_eq!(rgba(META_EYEBROW), (96, 96, 176, 255));
        assert_eq!(rgba(META_AUTHOR), (103, 182, 189, 255));
        let config = header_config();
        assert_eq!((config.padding.left, config.padding.right), (12, 16));
        assert_eq!((config.padding.top, config.padding.bottom), (12, 16));
        assert_eq!(config.border.width.bottom, 10);
    }

    #[test]
    fn snapshot_cells_keep_v2m_palette_roles() {
        let note = cell(48, b"C-3");
        let instrument = cell(4, b"04");
        let effect = cell(1, b"V40");
        let empty = cell(0, b"...");

        assert_eq!(
            rgba(cell_color(&note, RVColumnKind::Note as u32, false)),
            rgba(NOTE)
        );
        assert_eq!(
            rgba(cell_color(
                &instrument,
                RVColumnKind::Instrument as u32,
                false
            )),
            rgba(INSTRUMENT)
        );
        assert_eq!(
            rgba(cell_color(&effect, RVColumnKind::Effect as u32, false)),
            rgba(EFFECT)
        );
        assert_eq!(
            rgba(cell_color(&note, RVColumnKind::Note as u32, true)),
            rgba(TEXT_HIGHLIGHT)
        );
        assert_eq!(
            rgba(cell_color(&empty, RVColumnKind::Effect as u32, true)),
            rgba(TEXT_DIM)
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
