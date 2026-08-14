use flowi::{
    align, col, fixed, grow, BackgroundMode, Color, Image, ImageFormat, ImageHandle,
    ImageLoadOptions, Painter, RawStr, ResizeFilter,
};
use retrovert_host::visualization::VizSnapshot;

use crate::packed_faces;
use crate::pattern_display::{PatternDisplay, PatternDisplayColors, PatternDisplayConfig};
use crate::retro_font::{RetroFont, RetroFontConfig};
use crate::scope_display::ScopeDisplay;
use crate::view::View;
use crate::vu_display::VuDisplay;
use crate::BOX_RAISED_PATH;

const CHANNELS: usize = 4;
const VISIBLE_ROWS: i32 = 23;
const SCALED_WIDTH: f32 = 1_440.0;
const SCALED_HEIGHT: f32 = 1_080.0;
const FONT_SCALE: f32 = SCALED_WIDTH / 320.0;
const CHARSET: &[u8] = b" !#$%&'()*+,-./0123456789:;<=>?@ABCDEFGHIJKLMNOPQRSTUVWXYZ[\\]^_|";
const DOUBLE_CHARSET: &[u8] = b" ! #$%& ()*+,-./0123456789:;<=>?@ABCDEFGHIJKLMNOPQRSTUVWXYZ[\\]^_";
const BACKGROUND: Color = Color {
    r: 0,
    g: 0,
    b: 0x22,
    a: 255,
};
const NORMAL: Color = Color {
    r: 51,
    g: 68,
    b: 255,
    a: 255,
};
const CURRENT: Color = Color {
    r: 0,
    g: 0,
    b: 0,
    a: 255,
};
const HEADER: Color = Color {
    r: 187,
    g: 187,
    b: 187,
    a: 255,
};
const VALUE: Color = Color {
    r: 0,
    g: 0,
    b: 0,
    a: 255,
};
const FILL: Color = Color {
    r: 136,
    g: 136,
    b: 136,
    a: 255,
};
const HIGHLIGHT: Color = Color {
    r: 187,
    g: 187,
    b: 187,
    a: 255,
};
const SHADOW: Color = Color {
    r: 85,
    g: 85,
    b: 85,
    a: 255,
};
const SCOPE: Color = Color {
    r: 255,
    g: 221,
    b: 0,
    a: 255,
};
const TRANSPARENT: Color = Color {
    r: 0,
    g: 0,
    b: 0,
    a: 0,
};

pub struct PtView {
    font: RetroFont,
    font_2x: RetroFont,
    vu: VuDisplay,
    _box_raised: ImageHandle,
}

impl PtView {
    pub fn new(assets: Option<&flowi::Mount>) -> Option<Self> {
        let font = RetroFont::from_1bit_charset(
            packed_faces::PROTRACKER,
            CHARSET,
            RetroFontConfig {
                width: 512,
                height: 7,
                cell_width: 8,
                cell_height: 7,
                scale_x: FONT_SCALE,
                scale_y: FONT_SCALE,
            },
        )?;
        let font_2x = RetroFont::from_1bit_charset(
            packed_faces::PROTRACKER_DOUBLE_HEIGHT,
            DOUBLE_CHARSET,
            RetroFontConfig {
                width: 512,
                height: 14,
                cell_width: 8,
                cell_height: 14,
                scale_x: FONT_SCALE,
                scale_y: FONT_SCALE,
            },
        )?;
        let box_raised = assets.map_or(ImageHandle::INVALID, |mount| {
            Image::load_mount(
                mount,
                BOX_RAISED_PATH,
                ImageLoadOptions {
                    target_width: SCALED_WIDTH as i32,
                    target_height: 48,
                    max_width: 0,
                    max_height: 0,
                    format: ImageFormat::RGBA,
                    svg_scale_factor: 1.0,
                    svg_preserve_aspect_ratio: false,
                    detect_black_border: false,
                    preserve_aspect_ratio: false,
                    background_mode: BackgroundMode::Regular,
                    resize_filter: ResizeFilter::Nearest,
                    apply_vertical_fade: false,
                    fade_start: 0.0,
                    fade_r: 0,
                    fade_g: 0,
                    fade_b: 0,
                    override_cache_path: RawStr::EMPTY,
                },
            )
        });
        Some(Self {
            font,
            font_2x,
            vu: VuDisplay::new(320, (VISIBLE_ROWS / 2) * 7, CHANNELS as i32),
            _box_raised: box_raised,
        })
    }

    fn draw_header(&self, snapshot: &VizSnapshot, x: f32) {
        let scale = self.font.char_width() as f32 / 8.0;
        let row_height = 11.0 * scale;
        let text_y = (row_height - self.font.char_height() as f32) / 2.0 - scale;
        let (order, pattern) = snapshot
            .position
            .map(|position| (position.order, position.pattern))
            .unwrap_or((0, 0));
        let widths = [72.0, 34.0, 72.0, 34.0, 72.0, 37.0];
        let mut box_x = x;
        for width in widths {
            draw_box(box_x, scale, width * scale, row_height);
            box_x += width * scale;
        }
        self.shadow_text(x + 4.0 * scale, scale + text_y, b"POSITION", scale);
        self.font
            .draw_text(x + 73.0 * scale, scale + text_y, &decimal4(order), VALUE);
        self.shadow_text(x + 110.0 * scale, scale + text_y, b"PATTERN", scale);
        self.font
            .draw_text(x + 179.0 * scale, scale + text_y, &decimal4(pattern), VALUE);
        self.shadow_text(x + 218.0 * scale, scale + text_y, b"LENGTH", scale);
        self.font
            .draw_text(x + 285.0 * scale, scale + text_y, b"0000", VALUE);

        for (row, label) in [
            (12.0, b"SONGNAME:".as_slice()),
            (23.0, b"SAMPLENAME:".as_slice()),
        ] {
            let y = row * scale;
            draw_box(x, y, 321.0 * scale, row_height);
            let label_x = 92.0 * scale - scale - label.len() as f32 * self.font.char_width() as f32;
            self.shadow_text(x + label_x, y + text_y, label, scale);
        }
    }

    fn shadow_text(&self, x: f32, y: f32, text: &[u8], offset: f32) {
        self.font.draw_text(x + offset, y + offset, text, CURRENT);
        self.font.draw_text(x, y, text, HEADER);
    }

    fn draw_scopes(&mut self, snapshot: &VizSnapshot, x: f32) {
        let samples: [&[f32]; CHANNELS] =
            std::array::from_fn(|channel| snapshot.scope(channel).unwrap_or_default());
        ScopeDisplay::draw_channels(
            x + 30.0 * (SCALED_HEIGHT / 256.0),
            36.0 * (SCALED_HEIGHT / 256.0),
            310.0 * (SCALED_HEIGHT / 256.0),
            28.0 * (SCALED_HEIGHT / 256.0),
            &samples,
            &[SCOPE; CHANNELS],
            TRANSPARENT,
            TRANSPARENT,
            0.0,
        );
    }

    fn draw_contents(&mut self, snapshot: &VizSnapshot) {
        let x = 0.0;
        let scale = self.font.char_width() as f32 / 8.0;
        let header_height = 34.0 * scale;
        let scope_height = 32.0 * scale;
        let pattern_y = header_height + scope_height - 2.0 * scale;
        self.draw_header(snapshot, x);
        self.draw_scopes(snapshot, x);
        PatternDisplay::draw(
            x,
            pattern_y,
            SCALED_WIDTH,
            SCALED_HEIGHT - header_height - scope_height,
            &PatternDisplayConfig {
                visible_rows: VISIBLE_ROWS,
                font: Some(&self.font),
                font_2x: Some(&self.font_2x),
                colors: PatternDisplayColors {
                    normal: NORMAL,
                    current: CURRENT,
                    background: BACKGROUND,
                    box_fill: FILL,
                    box_highlight: HIGHLIGHT,
                    box_shadow: SHADOW,
                },
                use_double_height: true,
                draw_current_box: true,
            },
            snapshot,
        );
        let mut vu = [0_u8; CHANNELS];
        for (output, input) in vu.iter_mut().zip(snapshot.vu()) {
            *output = (input.clamp(0.0, 1.0) * 255.0) as u8;
        }
        let base_y = pattern_y + (VISIBLE_ROWS / 2) as f32 * self.font.char_height() as f32;
        self.vu.draw(
            x,
            base_y,
            (VISIBLE_ROWS / 2) as f32 * self.font.char_height() as f32 * 0.6,
            &vu,
            Color {
                r: 255,
                g: 255,
                b: 255,
                a: 220,
            },
            74.0 * scale,
            30.0 * scale,
            self.font.char_width() as f32 * 8.0,
        );
        self.draw_separators(x, pattern_y, scale);
    }

    fn draw_separators(&self, x: f32, pattern_y: f32, pixel: f32) {
        let top = 33.0 * pixel;
        draw_vsep(x, top, SCALED_HEIGHT, pixel);
        for channel in 0..=CHANNELS {
            let at = if channel == 0 {
                x + 24.0 * pixel
            } else if channel == CHANNELS {
                x + 318.0 * pixel
            } else {
                x + (24.0 + channel as f32 * 74.0) * pixel
            };
            draw_vsep(at, top, SCALED_HEIGHT, pixel);
        }
        let separator_y = pattern_y - 3.0 * pixel;
        draw_hsep(x + 3.0 * pixel, separator_y, 21.0 * pixel, pixel);
        for channel in 0..CHANNELS {
            let start = x + (27.0 + channel as f32 * 74.0) * pixel;
            let end = if channel + 1 == CHANNELS {
                x + 318.0 * pixel
            } else {
                x + (24.0 + (channel + 1) as f32 * 74.0) * pixel
            };
            draw_hsep(start, separator_y, end - start, pixel);
        }
        draw_hsep(x, SCALED_HEIGHT - 3.0 * pixel, 321.0 * pixel, pixel);
        let bottom = SCALED_HEIGHT - 3.0 * pixel;
        Painter::rect(x, bottom + pixel, pixel, pixel, HIGHLIGHT);
        Painter::rect(x, bottom + 2.0 * pixel, pixel, pixel, HIGHLIGHT);
        Painter::rect(x + pixel, bottom + 2.0 * pixel, pixel, pixel, FILL);
        let right = x + 318.0 * pixel;
        Painter::rect(right + 2.0 * pixel, bottom, pixel, pixel, SHADOW);
        Painter::rect(right + 2.0 * pixel, bottom + pixel, pixel, pixel, SHADOW);
        Painter::rect(right + pixel, bottom + pixel, pixel, pixel, SHADOW);
    }
}

fn decimal4(value: u32) -> [u8; 4] {
    let value = value % 10_000;
    [
        b'0' + ((value / 1_000) % 10) as u8,
        b'0' + ((value / 100) % 10) as u8,
        b'0' + ((value / 10) % 10) as u8,
        b'0' + (value % 10) as u8,
    ]
}

impl View for PtView {
    fn render(&mut self, snapshot: &VizSnapshot) {
        col! { width: grow!(), height: grow!(), color: [0, 0, 0], align: align!(Center, Center) {
            col! { width: fixed!(SCALED_WIDTH), height: fixed!(SCALED_HEIGHT), color: [0, 0, 34] {
                self.draw_contents(snapshot);
            }}
        }}
    }
}

fn draw_box(x: f32, y: f32, width: f32, height: f32) {
    Painter::rect(x, y, width, height, SHADOW);
    Painter::rect(x, y, width - 4.0, height - 4.0, HIGHLIGHT);
    Painter::rect(x + 4.0, y + 4.0, width - 8.0, height - 8.0, FILL);
}

fn draw_hsep(x: f32, y: f32, width: f32, pixel: f32) {
    Painter::rect(x, y, width, pixel, HIGHLIGHT);
    Painter::rect(x, y + pixel, width, pixel, FILL);
    Painter::rect(x, y + 2.0 * pixel, width, pixel, SHADOW);
}

fn draw_vsep(x: f32, y: f32, height: f32, pixel: f32) {
    Painter::rect(x, y, pixel, height, HIGHLIGHT);
    Painter::rect(x + pixel, y, pixel, height, FILL);
    Painter::rect(x + 2.0 * pixel, y, pixel, height, SHADOW);
}

///////////////////////////////////////////////////////////////////////////////////////////////////////////////////////

#[cfg(test)]
mod tests {
    #[test]
    fn c_ui_tree_oracle_is_the_centered_protracker_surface() {
        let oracle = include_str!("../tests/fixtures/music-player/ui_tree/pt.json");
        assert!(oracle.contains("\"width\":1440.00"));
        assert!(oracle.contains("\"height\":1080.00"));
    }
}
