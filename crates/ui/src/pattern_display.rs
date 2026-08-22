use flowi::{Color, Painter};
use retrovert_host::{
    ffi::playback::{RVColumnDesc, RVPatternCell, RVTrackerPosition},
    visualization::VizSnapshot,
};

use crate::retro_font::RetroFont;

const DEFAULT_VISIBLE_ROWS: i32 = 24;
const MAX_CHANNELS: usize = 10;
const NATIVE_CHAR_WIDTH: f32 = 8.0;
const ROW_COUNTER_X: f32 = 4.0;
const CHANNEL_TEXT_X: f32 = 30.0;
const CHANNEL_SPACING: f32 = 74.0;

#[derive(Clone, Copy, Debug)]
/// Pattern text and raised-box colors.
pub struct PatternDisplayColors {
    /// Non-current row text.
    pub normal: Color,
    /// Current row text.
    pub current: Color,
    /// Pattern area fill.
    pub background: Color,
    /// Raised-box interior.
    pub box_fill: Color,
    /// Raised-box top and left edges.
    pub box_highlight: Color,
    /// Raised-box bottom and right edges.
    pub box_shadow: Color,
}

impl Default for PatternDisplayColors {
    fn default() -> Self {
        Self {
            normal: Color {
                r: 200,
                g: 200,
                b: 200,
                a: 255,
            },
            current: Color {
                r: 255,
                g: 255,
                b: 128,
                a: 255,
            },
            background: Color {
                r: 0,
                g: 0,
                b: 34,
                a: 255,
            },
            box_fill: Color {
                r: 136,
                g: 136,
                b: 136,
                a: 255,
            },
            box_highlight: Color {
                r: 187,
                g: 187,
                b: 187,
                a: 255,
            },
            box_shadow: Color {
                r: 85,
                g: 85,
                b: 85,
                a: 255,
            },
        }
    }
}

#[derive(Clone, Copy, Debug)]
/// Font, row count, and highlight style for a pattern display.
pub struct PatternDisplayConfig<'a> {
    /// Rows drawn around the playhead.
    pub visible_rows: i32,
    /// Normal-row bitmap font.
    pub font: Option<&'a RetroFont>,
    /// Optional current-row bitmap font.
    pub font_2x: Option<&'a RetroFont>,
    /// Display colors.
    pub colors: PatternDisplayColors,
    /// Reserve twice the normal height for the current row.
    pub use_double_height: bool,
    /// Draw raised boxes behind current-row text.
    pub draw_current_box: bool,
}

impl Default for PatternDisplayConfig<'_> {
    fn default() -> Self {
        Self {
            visible_rows: DEFAULT_VISIBLE_ROWS,
            font: None,
            font_2x: None,
            colors: PatternDisplayColors::default(),
            use_double_height: false,
            draw_current_box: false,
        }
    }
}

/// Draws a v2 visualization snapshot as a tracker pattern.
pub struct PatternDisplay;

impl PatternDisplay {
    #[allow(clippy::too_many_arguments)]
    /// Draw the snapshot's advertised cell window around its playhead.
    pub fn draw(
        x: f32,
        y: f32,
        width: f32,
        height: f32,
        config: &PatternDisplayConfig<'_>,
        snapshot: &VizSnapshot,
    ) {
        let Some(position) = snapshot.position else {
            return;
        };
        let Some(font) = config.font else {
            return;
        };

        draw_snapshot(
            x,
            y,
            width,
            height,
            config,
            font,
            &snapshot.layout().columns,
            snapshot.layout().pattern_channels.len(),
            position,
            snapshot.cells(),
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_snapshot(
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    config: &PatternDisplayConfig<'_>,
    font: &RetroFont,
    columns: &[RVColumnDesc],
    pattern_channel_count: usize,
    position: RVTrackerPosition,
    cells: &[RVPatternCell],
) {
    let char_width = font.char_width();
    let char_height = font.char_height();
    if char_width == 0 || char_height == 0 {
        return;
    }

    let visible_rows = if config.visible_rows > 0 {
        config.visible_rows
    } else {
        DEFAULT_VISIBLE_ROWS
    };
    let channel_count = pattern_channel_count.min(MAX_CHANNELS);
    let column_count = columns.len();
    let scale = char_width as f32 / NATIVE_CHAR_WIDTH;
    let pixel = scale;
    let line_height = char_height as f32;
    let double_line_height = line_height * 2.0;
    let row_counter_x = ROW_COUNTER_X * scale;
    let channel_text_x = CHANNEL_TEXT_X * scale;
    let channel_spacing = CHANNEL_SPACING * scale;
    let char_width = char_width as f32;
    let half_visible = visible_rows / 2;

    Painter::rect(x, y, width, height, config.colors.background);

    let mut row_y = y;
    for visible_index in 0..visible_rows {
        let row = i64::from(position.row) - i64::from(half_visible) + i64::from(visible_index);
        let is_current = visible_index == half_visible;
        let use_double_height = is_current && config.use_double_height;
        let this_line_height = if use_double_height {
            double_line_height
        } else {
            line_height
        };
        let current_font = if use_double_height {
            config.font_2x.unwrap_or(font)
        } else {
            font
        };

        let Ok(row) = u32::try_from(row) else {
            row_y += this_line_height;
            continue;
        };
        if row < position.window_lo || row >= position.window_hi {
            row_y += this_line_height;
            continue;
        }

        let row_text = [hex_digit((row >> 4) as u8), hex_digit(row as u8)];
        let row_text_x = x + row_counter_x;
        let text_color = if is_current {
            config.colors.current
        } else {
            config.colors.normal
        };

        if is_current && config.draw_current_box {
            draw_raised_box(
                row_text_x - pixel,
                row_y,
                char_width * 2.0 + pixel * 2.0,
                this_line_height,
                pixel,
                config.colors,
            );
            for channel in 0..channel_count {
                let text_x = x + channel_text_x + channel as f32 * channel_spacing;
                let text_width = columns
                    .iter()
                    .map(|column| f32::from(column.char_width))
                    .sum::<f32>()
                    * char_width;
                draw_raised_box(
                    text_x - pixel * 2.0,
                    row_y,
                    text_width + pixel * 4.0,
                    this_line_height,
                    pixel,
                    config.colors,
                );
            }
        }

        let text_y = if is_current { row_y - pixel } else { row_y };
        current_font.draw_text(row_text_x, text_y, &row_text, text_color);

        let row_offset = (row - position.window_lo) as usize;
        for channel in 0..channel_count {
            let mut column_x = x + channel_text_x + channel as f32 * channel_spacing;
            for (column, descriptor) in columns.iter().enumerate() {
                let index = cell_index(
                    row_offset,
                    channel,
                    column,
                    pattern_channel_count,
                    column_count,
                );
                if let Some(cell) = cells.get(index) {
                    current_font.draw_text(column_x, text_y, cell_text(cell), text_color);
                }
                column_x += f32::from(descriptor.char_width) * char_width;
            }
        }

        row_y += this_line_height;
    }
}

pub(crate) fn cell_text(cell: &RVPatternCell) -> &[u8] {
    let length = cell
        .text
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(cell.text.len());
    &cell.text[..length]
}

fn cell_index(
    row: usize,
    channel: usize,
    column: usize,
    channel_count: usize,
    column_count: usize,
) -> usize {
    (row * channel_count + channel) * column_count + column
}

fn hex_digit(value: u8) -> u8 {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    HEX[usize::from(value & 0x0f)]
}

fn draw_raised_box(
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    border: f32,
    colors: PatternDisplayColors,
) {
    Painter::rect(
        x + border,
        y + border,
        width - border * 2.0,
        height - border * 2.0,
        colors.box_fill,
    );
    Painter::rect(x, y, width, border, colors.box_highlight);
    Painter::rect(x, y, border, height, colors.box_highlight);
    Painter::rect(x, y + height - border, width, border, colors.box_shadow);
    Painter::rect(x + width - border, y, border, height, colors.box_shadow);
}

///////////////////////////////////////////////////////////////////////////////////////////////////////////////////////

#[cfg(test)]
mod tests {
    use super::*;
    use retrovert_host::ffi::playback::RVColumnKind;

    const CHARSET: &[u8] = b"0123456789ABCDEF-=.#";

    fn test_font() -> RetroFont {
        let width = CHARSET.len() as i32 * 4;
        let mut bitmap = vec![0_u8; ((width + 7) / 8 * 5) as usize];
        for (character, byte) in CHARSET.iter().enumerate() {
            for y in 0..5 {
                for x in 0..4 {
                    if (byte.rotate_left(y as u32) >> x) & 1 != 0 {
                        let pixel = y * width as usize + character * 4 + x;
                        bitmap[pixel / 8] |= 1 << (7 - pixel % 8);
                    }
                }
            }
        }
        RetroFont::from_1bit_charset(
            &bitmap,
            CHARSET,
            crate::retro_font::RetroFontConfig {
                width,
                height: 5,
                cell_width: 4,
                cell_height: 5,
                scale_x: 2.0,
                scale_y: 2.0,
            },
        )
        .unwrap_or_else(|| panic!("valid pattern font was rejected"))
    }

    fn cell(text: &[u8]) -> RVPatternCell {
        let mut cell = RVPatternCell {
            raw: 0,
            text: [0; 16],
        };
        cell.text[..text.len()].copy_from_slice(text);
        cell
    }

    fn columns() -> [RVColumnDesc; 3] {
        [
            RVColumnDesc {
                label: [0; 16],
                char_width: 3,
                kind: RVColumnKind::Note as u32,
            },
            RVColumnDesc {
                label: [0; 16],
                char_width: 2,
                kind: RVColumnKind::Instrument as u32,
            },
            RVColumnDesc {
                label: [0; 16],
                char_width: 3,
                kind: RVColumnKind::Effect as u32,
            },
        ]
    }

    #[test]
    #[ignore = "embedded flowi capture must run serially"]
    fn capture_pattern_display_matches_blessed_png() {
        let mut font = None;
        let frame = crate::pixel_oracle::capture(180, 108, || {
            let font = font.get_or_insert_with(test_font);
            let config = PatternDisplayConfig {
                visible_rows: 3,
                font: Some(font),
                font_2x: Some(font),
                use_double_height: true,
                draw_current_box: true,
                ..PatternDisplayConfig::default()
            };
            let cells = [
                cell(b"C-4"),
                cell(b"01"),
                cell(b"A0F"),
                cell(b"E-4"),
                cell(b"03"),
                cell(b"C20"),
                cell(b"D#4"),
                cell(b"02"),
                cell(b"B10"),
                cell(b"F#4"),
                cell(b"04"),
                cell(b"D30"),
                cell(b"---"),
                cell(b"00"),
                cell(b"000"),
                cell(b"==="),
                cell(b"00"),
                cell(b"000"),
            ];
            draw_snapshot(
                8.0,
                8.0,
                164.0,
                92.0,
                &config,
                font,
                &columns(),
                2,
                RVTrackerPosition {
                    order: 0,
                    pattern: 0,
                    row: 17,
                    window_lo: 16,
                    window_hi: 19,
                },
                &cells,
            );
        });
        crate::pixel_oracle::assert_png("pattern_display", &frame);
    }

    #[test]
    fn cell_text_stops_at_nul_and_preserves_full_width() {
        assert_eq!(cell_text(&cell(b"C-4")), b"C-4");
        assert_eq!(cell_text(&cell(b"0123456789ABCDEF")), b"0123456789ABCDEF");
    }

    #[test]
    fn v2_cell_text_matches_legacy_formatter_fixtures() {
        let fixture = include_str!("../tests/fixtures/music-player/cell_text.tsv");
        for line in fixture.lines() {
            let fields: Vec<&str> = line.split('|').collect();
            assert_eq!(fields.len(), 4);
            let cells = [
                cell(fields[0].as_bytes()),
                cell(fields[1].as_bytes()),
                cell(fields[2].as_bytes()),
            ];
            let mut v2 = Vec::new();
            for value in &cells {
                v2.extend_from_slice(cell_text(value));
            }
            assert_eq!(v2, fields[3].as_bytes());
        }
    }

    #[test]
    fn cell_index_keeps_the_full_snapshot_stride() {
        assert_eq!(cell_index(1, 0, 0, 12, 3), 36);
        assert_eq!(cell_index(1, 9, 2, 12, 3), 65);
    }

    #[test]
    fn dependency_is_the_frozen_v2_abi() {
        assert_eq!(
            retrovert_host::ffi::playback::RV_PLAYBACK_PLUGIN_API_VERSION,
            2
        );
    }
}
