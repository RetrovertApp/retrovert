use flowi::{
    col, fit, fixed, grow, row, text_styled, Color, Font, FontId, Mount, Painter, TextConfig,
};
use retrovert_host::visualization::VizSnapshot;

use crate::scope_display::ScopeDisplay;
use crate::view::View;

const MAX_CHANNELS: usize = 64;
const SCALED_WIDTH: f32 = 1_920.0;
const SCALED_HEIGHT: f32 = 1_080.0;
const HEADER_HEIGHT: f32 = 80.0;
const SCOPE_PADDING: f32 = 4.0;
const VU_WIDTH: f32 = 12.0;
const FONT_SIZE: f32 = 18.0;

const HEADER_BACKGROUND: Color = color(0x1a, 0x1a, 0x28, 0xff);
const SCOPE_BACKGROUND: Color = color(0x08, 0x08, 0x10, 0xff);
const LABEL: Color = color(0x60, 0x60, 0x80, 0xff);
const VALUE: Color = color(0xa0, 0xb0, 0xd0, 0xff);
const SCOPE_LINE: Color = color(0x18, 0x18, 0x28, 0xff);
const SEPARATOR: Color = color(0x20, 0x20, 0x30, 0xff);
const CHANNEL_COLORS: [Color; 8] = [
    color(0x60, 0xa0, 0xe0, 0xff),
    color(0x60, 0xc0, 0x80, 0xff),
    color(0xe0, 0x90, 0x60, 0xff),
    color(0xc0, 0x70, 0xc0, 0xff),
    color(0xe0, 0xc0, 0x60, 0xff),
    color(0x70, 0xd0, 0xd0, 0xff),
    color(0xe0, 0x70, 0x70, 0xff),
    color(0x90, 0xe0, 0x90, 0xff),
];

const fn color(r: u8, g: u8, b: u8, a: u8) -> Color {
    Color { r, g, b, a }
}

pub struct GeneralView {
    font: FontId,
}

impl GeneralView {
    pub fn new(assets: Option<&Mount>) -> Self {
        let font = assets.map_or_else(
            || Font::load("fonts/JetBrainsMono-Regular.ttf", FONT_SIZE as u16),
            |mount| Font::load_mount(mount, "fonts/JetBrainsMono-Regular.ttf", FONT_SIZE as u16),
        );
        Self { font }
    }

    fn draw_header(&self) {
        row! { width: grow!(), height: fixed!(HEADER_HEIGHT), color: HEADER_BACKGROUND {
            row! { width: fit!(), height: grow!(), gap: 4 {
                col! { width: fit!(), height: fit!() {
                    text_styled("Title:", text(LABEL));
                    text_styled("Author:", text(LABEL));
                }}
                col! { width: fit!(), height: fit!() {
                    text_styled("", text(VALUE));
                    text_styled("", text(VALUE));
                }}
            }}
        }}
    }

    fn draw_channel(&self, snapshot: &VizSnapshot, channel: usize, grid: Grid) {
        let column = channel % grid.columns;
        let row = channel / grid.columns;
        let cell_width = (SCALED_WIDTH - SCOPE_PADDING) / grid.columns as f32;
        let available_height = SCALED_HEIGHT - HEADER_HEIGHT;
        let cell_height = (available_height - SCOPE_PADDING) / grid.rows as f32;
        let x = SCOPE_PADDING + column as f32 * cell_width;
        let y = HEADER_HEIGHT + SCOPE_PADDING + row as f32 * cell_height;
        let width = cell_width - SCOPE_PADDING;
        let height = cell_height - SCOPE_PADDING;
        let color = CHANNEL_COLORS[channel % CHANNEL_COLORS.len()];
        let name_height = FONT_SIZE + 2.0;
        let scope_y = y + name_height;
        let scope_height = height - name_height;

        let name = snapshot
            .layout
            .scope_channels
            .get(channel)
            .and_then(|description| channel_name(&description.name));
        match name {
            Some(name) => Painter::text(x + 4.0, y, name, self.font, color),
            None => {
                let fallback = ChannelLabel::new(channel);
                Painter::text(x + 4.0, y, fallback.as_str(), self.font, color);
            }
        }

        Painter::rect(x, scope_y, width, scope_height, SCOPE_BACKGROUND);
        Painter::rect(x, scope_y + scope_height / 2.0, width, 1.0, SCOPE_LINE);

        let vu_x = x + width - VU_WIDTH - 2.0;
        let scope_width = width - VU_WIDTH - 4.0;
        let vu = snapshot.vu().get(channel).copied().unwrap_or_default();
        if vu > 0.0 {
            let vu_height = vu.clamp(0.0, 1.0) * (scope_height - 4.0);
            let vu_y = scope_y + scope_height - 2.0 - vu_height;
            Painter::rect(vu_x, vu_y, VU_WIDTH, vu_height, Color { a: 0x80, ..color });
        }

        ScopeDisplay::draw_channel(
            x,
            scope_y,
            scope_width,
            scope_height,
            snapshot.scope(channel).unwrap_or_default(),
            color,
        );

        if column > 0 {
            Painter::rect(column as f32 * cell_width, y, 1.0, height, SEPARATOR);
        }
    }
}

impl View for GeneralView {
    fn render(&mut self, snapshot: &VizSnapshot) {
        self.draw_header();
        let channels = snapshot.layout.scope_channels.len().clamp(1, MAX_CHANNELS);
        let grid = Grid::for_channels(channels);
        for channel in 0..channels {
            self.draw_channel(snapshot, channel, grid);
        }
    }
}

fn text(color: Color) -> TextConfig {
    TextConfig {
        color,
        ..Default::default()
    }
}

fn channel_name(bytes: &[u8; 24]) -> Option<&str> {
    let length = bytes
        .iter()
        .position(|&byte| byte == 0)
        .unwrap_or(bytes.len());
    let name = std::str::from_utf8(&bytes[..length]).ok()?;
    (!name.is_empty()).then_some(name)
}

struct ChannelLabel {
    bytes: [u8; 5],
    length: usize,
}

impl ChannelLabel {
    fn new(channel: usize) -> Self {
        let number = channel + 1;
        let mut bytes = *b"Ch 00";
        let length = if number < 10 {
            bytes[3] = b'0' + number as u8;
            4
        } else {
            bytes[3] = b'0' + (number / 10) as u8;
            bytes[4] = b'0' + (number % 10) as u8;
            5
        };
        Self { bytes, length }
    }

    fn as_str(&self) -> &str {
        std::str::from_utf8(&self.bytes[..self.length]).unwrap_or_default()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Grid {
    columns: usize,
    rows: usize,
}

impl Grid {
    const fn for_channels(channels: usize) -> Self {
        match channels {
            1..=2 => Self {
                columns: channels,
                rows: 1,
            },
            3..=4 => Self {
                columns: 2,
                rows: 2,
            },
            5..=6 => Self {
                columns: 3,
                rows: 2,
            },
            7..=9 => Self {
                columns: 3,
                rows: 3,
            },
            10..=12 => Self {
                columns: 4,
                rows: 3,
            },
            13..=16 => Self {
                columns: 4,
                rows: 4,
            },
            17..=24 => Self {
                columns: 6,
                rows: channels.div_ceil(6),
            },
            _ => Self {
                columns: 8,
                rows: channels.div_ceil(8),
            },
        }
    }
}

///////////////////////////////////////////////////////////////////////////////////////////////////////////////////////

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grid_matches_the_c_view_at_every_boundary() {
        for (channels, columns, rows) in [
            (1, 1, 1),
            (2, 2, 1),
            (3, 2, 2),
            (4, 2, 2),
            (5, 3, 2),
            (6, 3, 2),
            (7, 3, 3),
            (9, 3, 3),
            (10, 4, 3),
            (12, 4, 3),
            (13, 4, 4),
            (16, 4, 4),
            (17, 6, 3),
            (24, 6, 4),
            (25, 8, 4),
            (32, 8, 4),
            (64, 8, 8),
        ] {
            assert_eq!(Grid::for_channels(channels), Grid { columns, rows });
        }
    }

    #[test]
    fn channel_names_are_borrowed_from_the_snapshot_description() {
        let mut named = [0; 24];
        named[..4].copy_from_slice(b"FM 1");
        assert_eq!(channel_name(&named), Some("FM 1"));
        assert_eq!(channel_name(&[0; 24]), None);
        assert_eq!(ChannelLabel::new(0).as_str(), "Ch 1");
        assert_eq!(ChannelLabel::new(63).as_str(), "Ch 64");
    }

    #[test]
    fn c_ui_tree_oracle_matches_the_ported_header_contract() {
        let oracle = include_str!("../tests/fixtures/music-player/ui_tree/general.json");
        assert!(oracle.contains("\"elementCount\":7"));
        assert!(oracle.contains("\"height\":80.00"));
        assert!(oracle.contains("\"backgroundColor\":{\"r\":26,\"g\":26,\"b\":40,\"a\":255}"));
        assert!(oracle.contains("\"text\":\"Title:\""));
        assert!(oracle.contains("\"text\":\"Author:\""));
        assert_eq!(HEADER_HEIGHT, 80.0);
        assert_eq!(rgba(HEADER_BACKGROUND), (26, 26, 40, 255));
        assert_eq!(rgba(LABEL), (96, 96, 128, 255));
        assert_eq!(rgba(VALUE), (160, 176, 208, 255));
    }

    fn rgba(color: Color) -> (u8, u8, u8, u8) {
        (color.r, color.g, color.b, color.a)
    }
}
