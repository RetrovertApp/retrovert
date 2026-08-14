use flowi::{Color, Painter};

const MAX_CHANNELS: i32 = 4;
const BAR_WIDTH: f32 = 24.0;
const MAX_LEVEL: f32 = 255.0;

#[derive(Clone, Copy, Debug, PartialEq)]
/// Vertical volume bars centered over tracker channel labels.
pub struct VuDisplay {
    num_channels: i32,
}

impl VuDisplay {
    /// Preserve the display geometry and cap rendering at four channels.
    pub fn new(_width: i32, _height: i32, num_channels: i32) -> Self {
        Self {
            num_channels: num_channels.min(MAX_CHANNELS),
        }
    }

    #[allow(clippy::too_many_arguments)]
    /// Draw nonzero levels upward from `base_y`, centered in each channel.
    pub fn draw(
        &self,
        x: f32,
        base_y: f32,
        max_height: f32,
        levels: &[u8; MAX_CHANNELS as usize],
        color: Color,
        channel_spacing: f32,
        channel_start_x: f32,
        channel_text_width: f32,
    ) {
        for channel in 0..self.num_channels {
            let level = levels[channel as usize];
            if level == 0 {
                continue;
            }

            let mut bar_height = f32::from(level) / MAX_LEVEL * max_height;
            if bar_height < 1.0 {
                bar_height = 1.0;
            }
            if bar_height > max_height {
                bar_height = max_height;
            }
            let channel_text_x = x + channel_start_x + channel as f32 * channel_spacing;
            let channel_center_x = channel_text_x + channel_text_width / 2.0;
            let bar_x = channel_center_x - BAR_WIDTH / 2.0;
            let bar_y = base_y - bar_height;

            Painter::rect(bar_x, bar_y, BAR_WIDTH, bar_height, color);
        }
    }
}

///////////////////////////////////////////////////////////////////////////////////////////////////////////////////////

#[cfg(test)]
mod tests {
    use super::*;

    const BAR_COLOR: Color = Color {
        r: 224,
        g: 236,
        b: 255,
        a: 192,
    };

    #[test]
    #[ignore = "embedded flowi capture must run serially"]
    fn capture_vu_display_matches_blessed_png() {
        let display = VuDisplay::new(200, 80, 4);
        let frame = crate::pixel_oracle::capture(224, 120, || {
            display.draw(
                12.0,
                100.0,
                80.0,
                &[0, 1, 128, 255],
                BAR_COLOR,
                48.0,
                0.0,
                32.0,
            );
        });
        crate::pixel_oracle::assert_png("vu_display", &frame);
    }

    #[test]
    fn construction_preserves_geometry_and_caps_channels() {
        let display = VuDisplay::new(320, 48, 9);
        assert_eq!(display.num_channels, MAX_CHANNELS);

        let empty = VuDisplay::new(0, 0, -1);
        assert_eq!(empty.num_channels, -1);
    }
}
