use flowi::{Color, Image, ImageFormat, ImageHandle, Painter, UiInput};

const MISSING_GLYPH: u8 = u8::MAX;

#[derive(Debug)]
/// A pre-scaled alpha atlas for byte-oriented tracker text.
pub struct RetroFont {
    atlas: ImageHandle,
    cell_width: i32,
    cell_height: i32,
    cells_per_row: i32,
    char_map: [u8; 256],
}

#[derive(Clone, Copy, Debug, PartialEq)]
/// Source atlas geometry and nearest-neighbor scale.
pub struct RetroFontConfig {
    /// Source atlas width in pixels.
    pub width: i32,
    /// Source atlas height in pixels.
    pub height: i32,
    /// Source glyph-cell width in pixels.
    pub cell_width: i32,
    /// Source glyph-cell height in pixels.
    pub cell_height: i32,
    /// Horizontal pre-scale factor.
    pub scale_x: f32,
    /// Vertical pre-scale factor.
    pub scale_y: f32,
}

impl RetroFont {
    /// Decode an MSB-first 1-bit atlas and map `charset` bytes in cell order.
    pub fn from_1bit_charset(data: &[u8], charset: &[u8], config: RetroFontConfig) -> Option<Self> {
        let RetroFontConfig {
            width,
            height,
            cell_width,
            cell_height,
            scale_x,
            scale_y,
        } = config;
        if data.is_empty() || charset.is_empty() {
            log::error!("retro font: invalid parameters");
            return None;
        }
        if width <= 0
            || height <= 0
            || cell_width <= 0
            || cell_height <= 0
            || scale_x <= 0.0
            || scale_y <= 0.0
        {
            log::error!("retro font: invalid dimensions");
            return None;
        }

        let source_stride = width.checked_add(7)?.checked_div(8)?;
        let source_len = usize::try_from(source_stride.checked_mul(height)?).ok()?;
        if data.len() < source_len {
            log::error!("retro font: bitmap is shorter than its dimensions");
            return None;
        }

        let scaled_width = (width as f32 * scale_x + 0.5) as i32;
        let scaled_height = (height as f32 * scale_y + 0.5) as i32;
        let scaled_cell_width = (cell_width as f32 * scale_x + 0.5) as i32;
        let scaled_cell_height = (cell_height as f32 * scale_y + 0.5) as i32;
        let atlas_len = usize::try_from(scaled_width.checked_mul(scaled_height)?).ok()?;
        let mut pixels = Vec::with_capacity(atlas_len);

        for destination_y in 0..scaled_height {
            let source_y = (destination_y as f32 / scale_y) as i32;
            for destination_x in 0..scaled_width {
                let source_x = (destination_x as f32 / scale_x) as i32;
                let byte_index = source_y * source_stride + (source_x >> 3);
                let bit_index = 7 - (source_x & 7);
                let byte_index = usize::try_from(byte_index).ok()?;
                pixels.push(if (data[byte_index] >> bit_index) & 1 != 0 {
                    255
                } else {
                    0
                });
            }
        }

        let cells_per_row = width / cell_width;
        let char_count = cells_per_row.checked_mul(height / cell_height)?;
        let mut char_map = [MISSING_GLYPH; 256];
        for (index, character) in charset
            .iter()
            .take(usize::try_from(char_count).ok()?)
            .enumerate()
        {
            char_map[usize::from(*character)] = index as u8;
        }

        let atlas = Image::from_pixels(&pixels, scaled_width, scaled_height, ImageFormat::Alpha);
        log::info!(
            "retro font: created {scaled_width}x{scaled_height} atlas, {} chars, cell {scaled_cell_width}x{scaled_cell_height} (scale {scale_x:.1}x{scale_y:.1})",
            charset.len()
        );
        Some(Self {
            atlas,
            cell_width: scaled_cell_width,
            cell_height: scaled_cell_height,
            cells_per_row,
            char_map,
        })
    }

    /// Draw bytes at `(x, y)`; missing glyphs advance without drawing.
    pub fn draw_text(&self, x: f32, y: f32, text: &[u8], color: Color) {
        let ui_scale = UiInput::get_scale();
        let inverse_scale = if ui_scale > 0.0 {
            ui_scale.recip()
        } else {
            1.0
        };
        let source_width = self.cell_width as f32;
        let source_height = self.cell_height as f32;
        let destination_width = source_width * inverse_scale;
        let destination_height = source_height * inverse_scale;
        let mut cursor_x = x;

        for character in text {
            let index = self.char_map[usize::from(*character)];
            if index != MISSING_GLYPH {
                let index = i32::from(index);
                let source_x = (index % self.cells_per_row) * self.cell_width;
                let source_y = (index / self.cells_per_row) * self.cell_height;
                Painter::image_sub(
                    cursor_x,
                    y,
                    destination_width,
                    destination_height,
                    source_x as f32,
                    source_y as f32,
                    source_width,
                    source_height,
                    self.atlas,
                    color,
                );
            }
            cursor_x += destination_width;
        }
    }

    /// Pre-scaled glyph-cell width.
    pub fn char_width(&self) -> i32 {
        self.cell_width
    }

    /// Pre-scaled glyph-cell height.
    pub fn char_height(&self) -> i32 {
        self.cell_height
    }
}

///////////////////////////////////////////////////////////////////////////////////////////////////////////////////////

#[cfg(test)]
mod tests {
    use super::*;

    const FONT_DATA: [u8; 4] = [0xf8, 0xf0, 0xf0, 0xf0];
    const TINT: Color = Color {
        r: 200,
        g: 100,
        b: 50,
        a: 255,
    };
    const CONFIG: RetroFontConfig = RetroFontConfig {
        width: 8,
        height: 4,
        cell_width: 4,
        cell_height: 4,
        scale_x: 2.0,
        scale_y: 2.0,
    };

    #[test]
    #[ignore = "embedded flowi capture must run serially"]
    fn capture_retro_font_matches_blessed_png() {
        let mut font = None;
        let frame = crate::pixel_oracle::capture(96, 48, || {
            let font = font.get_or_insert_with(|| {
                RetroFont::from_1bit_charset(&FONT_DATA, b"AB", CONFIG)
                    .unwrap_or_else(|| panic!("valid synthetic font was rejected"))
            });
            assert_eq!((font.char_width(), font.char_height()), (8, 8));
            font.draw_text(12.0, 12.0, b"AB?A", TINT);
        });
        crate::pixel_oracle::assert_png("retro_font", &frame);
    }

    #[test]
    fn rejects_invalid_input_before_upload() {
        assert!(RetroFont::from_1bit_charset(&[], b"AB", CONFIG).is_none());
        assert!(RetroFont::from_1bit_charset(
            &FONT_DATA,
            b"AB",
            RetroFontConfig { width: 0, ..CONFIG }
        )
        .is_none());
        assert!(RetroFont::from_1bit_charset(&FONT_DATA[..3], b"AB", CONFIG).is_none());
    }
}
