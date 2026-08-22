use flowi::{Color, Font, FontId, Mount, Painter};
use retrovert_host::{
    ffi::playback::{RVColumnDesc, RVColumnKind, RVPatternCell},
    visualization::VizSnapshot,
};

use crate::packed_faces;
use crate::pattern_display::cell_text;
use crate::retro_font::{RetroFont, RetroFontConfig};
use crate::scope_display::ScopeDisplay;
use crate::view::View;
use crate::MONO_FONT_PATH;

const MAX_CHANNELS: usize = 64;
const WIDTH: f32 = 1_920.0;
const HEIGHT: f32 = 1_080.0;
const HEADER_HEIGHT: f32 = 40.0;
const SCOPE_HEIGHT: f32 = 60.0;
const CHANNEL_HEADER_HEIGHT: f32 = 24.0;
const HEADER_FONT_SIZE: u16 = 24;
const ROW_NUMBER_CHARS: i32 = 3;

const ROW_ODD: Color = color(0x00, 0x00, 0x00);
const ROW_EVEN: Color = color(0x10, 0x14, 0x2c);
const BACKGROUND: Color = color(0x0a, 0x0e, 0x28);
const HEADER_BACKGROUND: Color = color(0x10, 0x18, 0x38);
const ROW_CURRENT: Color = color(0x00, 0x70, 0x90);
const TEXT_NOTE: Color = color(0xd0, 0xd0, 0xd0);
const TEXT_DATA: Color = color(0x80, 0x90, 0xa0);
const TEXT_EFFECT: Color = color(0x60, 0x80, 0x90);
const TEXT_DIM: Color = color(0x30, 0x40, 0x50);
const TEXT_HIGHLIGHT: Color = color(0xff, 0xff, 0xff);
const TEXT_HEADER: Color = color(0x90, 0xb8, 0xd0);
const ROW_NUMBER: Color = color(0x70, 0x80, 0x90);
const SEPARATOR: Color = color(0x20, 0x30, 0x50);
const SCOPE: Color = color(0x80, 0xd0, 0xff);
const TRANSPARENT: Color = Color {
    r: 0,
    g: 0,
    b: 0,
    a: 0,
};

const FONT3_CHARSET: &[u8] = b"0123456789ABCDEFG                   -#b.===";
const FONT4_CHARSET: &[u8] =
    b"0123456789ABCDEFG                   -#b.=                                     ";
const FONT5_CHARSET: &[u8] = b"0123456789ABCDEFG                   -#b";

const fn color(r: u8, g: u8, b: u8) -> Color {
    Color { r, g, b, a: 255 }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DisplayMode {
    Full,
    NoVolume,
    Compact,
    Minimal,
}

struct Layout<'a> {
    channels: usize,
    note_font: &'a RetroFont,
    data_font: &'a RetroFont,
    row_font: &'a RetroFont,
    mode: DisplayMode,
    row_number_width: f32,
    visible_rows: i32,
}

pub struct FastTrackerView {
    font: RetroFont,
    font_small: RetroFont,
    font_tiny: RetroFont,
    header_font: FontId,
}

impl FastTrackerView {
    pub fn new(assets: Option<&Mount>) -> Option<Self> {
        let font = create_font(packed_faces::FT2_LARGE, FONT5_CHARSET, 624, 8, 16, 8)?;
        let font_small = create_font(packed_faces::FT2_SMALL, FONT4_CHARSET, 624, 8, 8, 8)?;
        let font_tiny = create_font(packed_faces::FT2_TINY, FONT3_CHARSET, 172, 7, 4, 7)?;
        let header_font = assets.map_or_else(
            || Font::load(MONO_FONT_PATH, HEADER_FONT_SIZE),
            |mount| Font::load_mount(mount, MONO_FONT_PATH, HEADER_FONT_SIZE),
        );
        Some(Self {
            font,
            font_small,
            font_tiny,
            header_font,
        })
    }

    fn layout(&self, channel_count: usize, columns: &[RVColumnDesc]) -> Layout<'_> {
        let channels = channel_count.clamp(1, MAX_CHANNELS);
        let (note_font, data_font, row_font) = match channels {
            1..=4 => (&self.font, &self.font_small, &self.font_small),
            5..=8 => (&self.font_small, &self.font_small, &self.font_small),
            _ => (&self.font_tiny, &self.font_tiny, &self.font_tiny),
        };
        let row_number_width = (ROW_NUMBER_CHARS + 1) as f32 * row_font.char_width() as f32;
        let mode = calculate_display_mode(
            (WIDTH - row_number_width) as i32,
            channels,
            note_font.char_width(),
            data_font.char_width(),
            columns,
        );
        let data_font = if channels <= 4 && mode != DisplayMode::Full {
            note_font
        } else {
            data_font
        };
        let available_height = HEIGHT - HEADER_HEIGHT - SCOPE_HEIGHT - CHANNEL_HEADER_HEIGHT;
        let visible_rows = (available_height / note_font.char_height() as f32) as i32;
        Layout {
            channels,
            note_font,
            data_font,
            row_font,
            mode,
            row_number_width,
            visible_rows: visible_rows.max(1),
        }
    }

    fn draw_header(&self, channels: usize) {
        Painter::rect(0.0, 0.0, WIDTH, HEADER_HEIGHT, HEADER_BACKGROUND);
        let y = (HEADER_HEIGHT - f32::from(HEADER_FONT_SIZE)) / 2.0;
        Painter::text(8.0, y, "FastTracker", self.header_font, TEXT_HEADER);
        let info = HeaderInfo::new(channels);
        let estimated_width = info.as_str().len() as f32 * f32::from(HEADER_FONT_SIZE) * 0.6;
        Painter::text(
            WIDTH - estimated_width - 8.0,
            y,
            info.as_str(),
            self.header_font,
            TEXT_HEADER,
        );
    }

    fn draw_scopes(&self, snapshot: &VizSnapshot, layout: &Layout<'_>, y: f32) {
        let mut samples: [&[f32]; MAX_CHANNELS] = [&[]; MAX_CHANNELS];
        for (channel, output) in samples.iter_mut().enumerate().take(layout.channels) {
            *output = snapshot.scope(channel).unwrap_or_default();
        }
        let colors = [SCOPE; MAX_CHANNELS];
        let separator = if layout.channels <= 40 {
            SEPARATOR
        } else {
            TRANSPARENT
        };
        ScopeDisplay::draw_channels(
            layout.row_number_width,
            y,
            WIDTH - layout.row_number_width,
            SCOPE_HEIGHT - 4.0,
            &samples[..layout.channels],
            &colors[..layout.channels],
            BACKGROUND,
            separator,
            2.0,
        );
    }

    fn draw_channel_headers(&self, layout: &Layout<'_>, y: f32) {
        let channel_width = (WIDTH - layout.row_number_width) / layout.channels as f32;
        for channel in 0..layout.channels {
            let label = decimal2(channel + 1);
            layout.row_font.draw_text(
                layout.row_number_width + channel as f32 * channel_width + 2.0,
                y,
                &label,
                TEXT_HEADER,
            );
        }
    }

    fn draw_pattern(&self, snapshot: &VizSnapshot, layout: &Layout<'_>, y: f32) {
        let Some(position) = snapshot.position else {
            return;
        };
        let half_visible = layout.visible_rows / 2;
        let line_height = layout.note_font.char_height() as f32;
        let channel_width = (WIDTH - layout.row_number_width) / layout.channels as f32;
        let column_count = snapshot.layout().columns.len();
        let snapshot_channels = snapshot.layout().pattern_channels.len();

        for visible_row in 0..layout.visible_rows {
            let row = i64::from(position.row) - i64::from(half_visible) + i64::from(visible_row);
            let row_y = y + visible_row as f32 * line_height;
            let current = visible_row == half_visible;
            let background = if current {
                ROW_CURRENT
            } else if row % 2 == 0 {
                ROW_EVEN
            } else {
                ROW_ODD
            };
            Painter::rect(0.0, row_y, WIDTH, line_height, background);

            let valid_row = u32::try_from(row)
                .ok()
                .filter(|row| *row >= position.window_lo && *row < position.window_hi);
            let row_label = valid_row.map_or(RowLabel::empty(), RowLabel::hex);
            layout.row_font.draw_text(
                2.0,
                row_y,
                row_label.as_bytes(),
                if current { TEXT_HIGHLIGHT } else { ROW_NUMBER },
            );

            for channel in 0..layout.channels {
                let mut cell_x = layout.row_number_width + channel as f32 * channel_width + 2.0;
                if let Some(row) = valid_row {
                    let row_offset = (row - position.window_lo) as usize;
                    for (column, descriptor) in snapshot.layout().columns.iter().enumerate() {
                        if !column_visible(descriptor.kind, layout.mode) {
                            continue;
                        }
                        let index =
                            (row_offset * snapshot_channels + channel) * column_count + column;
                        let Some(cell) = snapshot.cells().get(index) else {
                            continue;
                        };
                        let note = descriptor.kind == RVColumnKind::Note as u32;
                        let font = if note {
                            layout.note_font
                        } else {
                            layout.data_font
                        };
                        let color = if current {
                            TEXT_HIGHLIGHT
                        } else {
                            cell_color(descriptor.kind, cell)
                        };
                        font.draw_text(cell_x, row_y, cell_text(cell), color);
                        cell_x += f32::from(descriptor.char_width) * font.char_width() as f32;
                    }
                } else {
                    layout.note_font.draw_text(cell_x, row_y, b"---", TEXT_DIM);
                }
                if channel > 0 && layout.channels <= 40 {
                    let separator_x = layout.row_number_width + channel as f32 * channel_width;
                    Painter::rect(separator_x, row_y, 1.0, line_height, SEPARATOR);
                }
            }
        }
    }
}

impl View for FastTrackerView {
    fn render(&mut self, snapshot: &VizSnapshot) {
        let layout = self.layout(
            snapshot.layout().pattern_channels.len(),
            &snapshot.layout().columns,
        );
        Painter::rect(0.0, 0.0, WIDTH, HEIGHT, BACKGROUND);
        self.draw_header(layout.channels);
        self.draw_scopes(snapshot, &layout, HEADER_HEIGHT);
        self.draw_channel_headers(&layout, HEADER_HEIGHT + SCOPE_HEIGHT);
        self.draw_pattern(
            snapshot,
            &layout,
            HEADER_HEIGHT + SCOPE_HEIGHT + CHANNEL_HEADER_HEIGHT,
        );
    }
}

fn create_font(
    data: &[u8],
    charset: &[u8],
    width: i32,
    height: i32,
    cell_width: i32,
    cell_height: i32,
) -> Option<RetroFont> {
    RetroFont::from_1bit_charset(
        data,
        charset,
        RetroFontConfig {
            width,
            height,
            cell_width,
            cell_height,
            scale_x: 2.0,
            scale_y: 2.0,
        },
    )
}

fn calculate_display_mode(
    available_width: i32,
    channels: usize,
    large_char_width: i32,
    small_char_width: i32,
    columns: &[RVColumnDesc],
) -> DisplayMode {
    let separator = small_char_width;
    let data_char_width = if channels <= 4 {
        large_char_width
    } else {
        small_char_width
    };
    let channels = i32::try_from(channels).unwrap_or(i32::MAX);
    for mode in [
        DisplayMode::Full,
        DisplayMode::NoVolume,
        DisplayMode::Compact,
    ] {
        let width = columns
            .iter()
            .filter(|column| column_visible(column.kind, mode))
            .map(|column| {
                let font_width = if column.kind == RVColumnKind::Note as u32 {
                    large_char_width
                } else if mode == DisplayMode::Full {
                    small_char_width
                } else {
                    data_char_width
                };
                i32::from(column.char_width) * font_width
            })
            .sum::<i32>()
            + separator;
        if width.saturating_mul(channels) <= available_width {
            return mode;
        }
    }
    DisplayMode::Minimal
}

fn column_visible(kind: u32, mode: DisplayMode) -> bool {
    if kind == RVColumnKind::Note as u32 {
        return true;
    }
    if kind == RVColumnKind::Instrument as u32 {
        return mode != DisplayMode::Minimal;
    }
    if kind == RVColumnKind::Volume as u32 {
        return mode == DisplayMode::Full;
    }
    if kind == RVColumnKind::Effect as u32 || kind == RVColumnKind::Param as u32 {
        return matches!(mode, DisplayMode::Full | DisplayMode::NoVolume);
    }
    mode == DisplayMode::Full
}

fn cell_color(kind: u32, cell: &RVPatternCell) -> Color {
    if cell_text(cell)
        .iter()
        .all(|byte| matches!(byte, b'-' | b'.' | b'0'))
    {
        return TEXT_DIM;
    }
    if kind == RVColumnKind::Note as u32 {
        TEXT_NOTE
    } else if kind == RVColumnKind::Effect as u32 || kind == RVColumnKind::Param as u32 {
        TEXT_EFFECT
    } else {
        TEXT_DATA
    }
}

fn decimal2(value: usize) -> [u8; 2] {
    let value = value % 100;
    [b'0' + (value / 10) as u8, b'0' + (value % 10) as u8]
}

struct HeaderInfo {
    bytes: [u8; 16],
    length: usize,
}

impl HeaderInfo {
    fn new(channels: usize) -> Self {
        let mut bytes = [0; 16];
        bytes[..3].copy_from_slice(b"Ch:");
        let mut length = 3;
        length += write_decimal(&mut bytes[length..], channels);
        bytes[length..length + 6].copy_from_slice(b" Pos:0");
        length += 6;
        Self { bytes, length }
    }

    fn as_str(&self) -> &str {
        std::str::from_utf8(&self.bytes[..self.length]).unwrap_or_default()
    }
}

fn write_decimal(output: &mut [u8], value: usize) -> usize {
    let tens = value / 10;
    if tens > 0 {
        output[0] = b'0' + tens as u8;
        output[1] = b'0' + (value % 10) as u8;
        2
    } else {
        output[0] = b'0' + value as u8;
        1
    }
}

struct RowLabel {
    bytes: [u8; 8],
    start: usize,
}

impl RowLabel {
    fn empty() -> Self {
        let mut bytes = [0; 8];
        bytes[..2].copy_from_slice(b"--");
        Self { bytes, start: 0 }
    }

    fn hex(mut value: u32) -> Self {
        let mut bytes = [0; 8];
        let mut start = bytes.len();
        loop {
            start -= 1;
            bytes[start] = b"0123456789ABCDEF"[(value & 0x0f) as usize];
            value >>= 4;
            if value == 0 && start <= bytes.len() - 2 {
                break;
            }
        }
        Self { bytes, start }
    }

    fn as_bytes(&self) -> &[u8] {
        let end = if self.start == 0 && self.bytes[0] == b'-' {
            2
        } else {
            self.bytes.len()
        };
        &self.bytes[self.start..end]
    }
}

///////////////////////////////////////////////////////////////////////////////////////////////////////////////////////

#[cfg(test)]
mod tests {
    use super::*;

    fn columns() -> [RVColumnDesc; 5] {
        [
            column(RVColumnKind::Note, 3),
            column(RVColumnKind::Instrument, 2),
            column(RVColumnKind::Volume, 3),
            column(RVColumnKind::Effect, 1),
            column(RVColumnKind::Param, 2),
        ]
    }

    fn column(kind: RVColumnKind, char_width: u8) -> RVColumnDesc {
        RVColumnDesc {
            label: [0; 16],
            char_width,
            kind: kind as u32,
        }
    }

    #[test]
    fn auto_layout_matches_the_c_width_thresholds() {
        let columns = columns();
        assert_eq!(
            calculate_display_mode(1_856, 4, 32, 16, &columns),
            DisplayMode::Full
        );
        assert_eq!(
            calculate_display_mode(1_000, 4, 32, 16, &columns),
            DisplayMode::Full
        );
        assert_eq!(
            calculate_display_mode(1_856, 8, 16, 16, &columns),
            DisplayMode::Full
        );
        assert_eq!(
            calculate_display_mode(1_888, 16, 8, 8, &columns),
            DisplayMode::Full
        );
        assert_eq!(
            calculate_display_mode(1_888, 32, 8, 8, &columns),
            DisplayMode::Compact
        );
        assert_eq!(
            calculate_display_mode(1_888, 64, 8, 8, &columns),
            DisplayMode::Minimal
        );
    }

    #[test]
    fn full_mode_accounts_for_the_v2_three_character_volume() {
        assert_eq!(
            calculate_display_mode(1_888, 21, 8, 8, &columns()),
            DisplayMode::NoVolume
        );
    }

    #[test]
    fn display_modes_keep_the_c_column_contract() {
        assert!(column_visible(
            RVColumnKind::Volume as u32,
            DisplayMode::Full
        ));
        assert!(!column_visible(
            RVColumnKind::Volume as u32,
            DisplayMode::NoVolume
        ));
        assert!(column_visible(
            RVColumnKind::Effect as u32,
            DisplayMode::NoVolume
        ));
        assert!(!column_visible(
            RVColumnKind::Effect as u32,
            DisplayMode::Compact
        ));
        assert!(column_visible(
            RVColumnKind::Instrument as u32,
            DisplayMode::Compact
        ));
        assert!(!column_visible(
            RVColumnKind::Instrument as u32,
            DisplayMode::Minimal
        ));
    }

    #[test]
    fn c_ui_tree_oracle_is_the_painter_only_surface() {
        let oracle = include_str!("../tests/fixtures/music-player/ui_tree/fasttracker.json");
        assert!(oracle.contains("\"type\":\"custom\""));
        assert!(oracle.contains("\"width\":1920.00"));
        assert!(oracle.contains("\"height\":1080.00"));
        assert!(oracle.contains("\"elementCount\":1"));
    }

    #[test]
    fn row_and_header_labels_match_the_c_format() {
        assert_eq!(RowLabel::hex(0).as_bytes(), b"00");
        assert_eq!(RowLabel::hex(15).as_bytes(), b"0F");
        assert_eq!(RowLabel::hex(256).as_bytes(), b"100");
        assert_eq!(RowLabel::empty().as_bytes(), b"--");
        assert_eq!(HeaderInfo::new(8).as_str(), "Ch:8 Pos:0");
        assert_eq!(HeaderInfo::new(64).as_str(), "Ch:64 Pos:0");
    }
}
