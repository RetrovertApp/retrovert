use flowi::{Color, Painter};

const TRIGGER_TEMPLATE_SIZE: usize = 64;
const MAX_CROSSINGS: usize = 128;
const MAX_CHANNELS: usize = 32;

#[derive(Clone)]
struct TriggerState {
    template: [f32; TRIGGER_TEMPLATE_SIZE],
    initialized: bool,
}

impl Default for TriggerState {
    fn default() -> Self {
        Self {
            template: [0.0; TRIGGER_TEMPLATE_SIZE],
            initialized: false,
        }
    }
}

/// Per-channel trigger state for waveform rendering.
pub struct ScopeDisplay {
    triggers: [TriggerState; MAX_CHANNELS],
}

impl Default for ScopeDisplay {
    fn default() -> Self {
        Self {
            triggers: std::array::from_fn(|_| TriggerState::default()),
        }
    }
}

impl ScopeDisplay {
    /// Draw samples linearly across one channel area.
    pub fn draw_channel(x: f32, y: f32, width: f32, height: f32, samples: &[f32], color: Color) {
        if samples.is_empty() || width <= 0.0 || height <= 0.0 {
            return;
        }

        draw_samples(x, y, width, height, samples, 0, samples.len(), color);
    }

    /// Draw one channel around a tracked falling-edge trigger.
    #[allow(clippy::too_many_arguments)]
    pub fn draw_channel_oscilloscope(
        &mut self,
        x: f32,
        y: f32,
        width: f32,
        height: f32,
        samples: &[f32],
        color: Color,
        channel: usize,
    ) {
        if samples.len() < 5 || width <= 0.0 || height <= 0.0 {
            return;
        }

        let trigger = self.find_trigger(samples, channel);
        let display_len = samples.len() / 5;
        let mut display_start = trigger.saturating_sub(display_len / 4);
        let mut display_end = display_start + display_len;

        if display_end > samples.len() {
            display_end = samples.len();
            display_start = samples.len().saturating_sub(display_len);
        }

        draw_samples(
            x,
            y,
            width,
            height,
            samples,
            display_start,
            display_end,
            color,
        );
    }

    /// Draw raw channel scopes side by side.
    #[allow(clippy::too_many_arguments)]
    pub fn draw_channels(
        x: f32,
        y: f32,
        width: f32,
        height: f32,
        samples: &[&[f32]],
        colors: &[Color],
        background: Color,
        separator: Color,
        padding: f32,
    ) {
        draw_channels_impl(
            x,
            y,
            width,
            height,
            samples,
            colors,
            background,
            separator,
            padding,
            |channel_x, channel_y, channel_width, channel_height, channel_samples, color, _| {
                Self::draw_channel(
                    channel_x,
                    channel_y,
                    channel_width,
                    channel_height,
                    channel_samples,
                    color,
                );
            },
        );
    }

    /// Draw triggered channel scopes side by side.
    #[allow(clippy::too_many_arguments)]
    pub fn draw_channels_oscilloscope(
        &mut self,
        x: f32,
        y: f32,
        width: f32,
        height: f32,
        samples: &[&[f32]],
        colors: &[Color],
        background: Color,
        separator: Color,
        padding: f32,
    ) {
        draw_channels_impl(
            x,
            y,
            width,
            height,
            samples,
            colors,
            background,
            separator,
            padding,
            |channel_x,
             channel_y,
             channel_width,
             channel_height,
             channel_samples,
             color,
             channel| {
                self.draw_channel_oscilloscope(
                    channel_x,
                    channel_y,
                    channel_width,
                    channel_height,
                    channel_samples,
                    color,
                    channel,
                );
            },
        );
    }

    fn find_trigger(&mut self, samples: &[f32], channel: usize) -> usize {
        if samples.len() < 16 || channel >= MAX_CHANNELS {
            return 0;
        }

        let display_len = samples.len() / 5;
        let center = samples.len() / 2;
        let mut minimum = samples[0];
        let mut maximum = samples[0];
        for sample in &samples[1..] {
            minimum = minimum.min(*sample);
            maximum = maximum.max(*sample);
        }

        if maximum - minimum < 0.001 {
            return center;
        }

        let trigger_level = (minimum + maximum) * 0.5;
        let search_start = display_len / 2;
        let search_end = samples.len() - display_len / 2;
        let mut crossings = [0; MAX_CROSSINGS];
        let mut crossing_count = 0;
        let mut above = samples[search_start] >= trigger_level;

        for (index, sample) in samples
            .iter()
            .enumerate()
            .take(search_end)
            .skip(search_start + 1)
        {
            let now_above = *sample >= trigger_level;
            if above && !now_above {
                crossings[crossing_count] = index;
                crossing_count += 1;
                if crossing_count == MAX_CROSSINGS {
                    break;
                }
            }
            above = now_above;
        }

        if crossing_count == 0 {
            return center;
        }

        let state = &mut self.triggers[channel];
        let crossings = &crossings[..crossing_count];
        let result = if crossing_count == 1 {
            crossings[0]
        } else if state.initialized {
            let half_template = TRIGGER_TEMPLATE_SIZE / 2;
            let correlation_start = search_start + half_template;
            let correlation_end = search_end.saturating_sub(half_template);
            let mut best_score = 1.0e30_f32;
            let mut best_position = center;

            if correlation_start < correlation_end {
                for position in correlation_start..correlation_end {
                    let mut score = 0.0;
                    for (sample, template) in samples
                        [position - half_template..position + half_template]
                        .iter()
                        .zip(&state.template)
                    {
                        let difference = sample - template;
                        score += difference * difference;
                    }
                    if score < best_score {
                        best_score = score;
                        best_position = position;
                    }
                }
            }

            nearest_crossing(crossings, best_position)
        } else {
            nearest_crossing(crossings, center)
        };

        if samples.len() >= TRIGGER_TEMPLATE_SIZE {
            let template_start = result
                .saturating_sub(TRIGGER_TEMPLATE_SIZE / 2)
                .min(samples.len() - TRIGGER_TEMPLATE_SIZE);
            state
                .template
                .copy_from_slice(&samples[template_start..template_start + TRIGGER_TEMPLATE_SIZE]);
            state.initialized = true;
        }
        result
    }
}

fn nearest_crossing(crossings: &[usize], target: usize) -> usize {
    let mut best_distance = usize::MAX;
    let mut result = crossings[0];
    for crossing in crossings {
        let distance = crossing.abs_diff(target);
        if distance < best_distance {
            best_distance = distance;
            result = *crossing;
        }
    }
    result
}

#[allow(clippy::too_many_arguments)]
fn draw_samples(
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    samples: &[f32],
    start: usize,
    end: usize,
    color: Color,
) {
    let middle_y = y + height / 2.0;
    let half_height = height / 2.0 - 2.0;
    let count = end - start;
    let sample_step = count as f32 / width;
    let mut previous_y: f32 = -1.0;

    for pixel_x in 0..width as i32 {
        let mut sample_index = start + (pixel_x as f32 * sample_step) as usize;
        if sample_index >= end {
            sample_index = end - 1;
        }

        let sample = samples[sample_index].clamp(-1.0, 1.0);
        let point_y = middle_y - sample * half_height;
        if previous_y >= 0.0 {
            let y0 = previous_y.min(point_y);
            let y1 = previous_y.max(point_y);
            Painter::rect(x + pixel_x as f32, y0, 2.0, y1 - y0 + 1.0, color);
        } else {
            Painter::rect(x + pixel_x as f32, point_y, 2.0, 2.0, color);
        }
        previous_y = point_y;
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_channels_impl(
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    samples: &[&[f32]],
    colors: &[Color],
    background: Color,
    separator: Color,
    padding: f32,
    mut draw: impl FnMut(f32, f32, f32, f32, &[f32], Color, usize),
) {
    let channel_count = samples.len().min(colors.len());
    if channel_count == 0 {
        return;
    }

    let channel_width = width / channel_count as f32;
    for channel in 0..channel_count {
        let channel_x = x + channel as f32 * channel_width + padding;
        let scope_width = channel_width - padding * 2.0;
        if background.a > 0 {
            Painter::rect(channel_x, y, scope_width, height, background);
        }
        if !samples[channel].is_empty() {
            draw(
                channel_x,
                y,
                scope_width,
                height,
                samples[channel],
                colors[channel],
                channel,
            );
        }
        if separator.a > 0 && channel > 0 {
            let separator_x = x + channel as f32 * channel_width;
            Painter::rect(separator_x - 1.0, y, 1.0, height, separator);
        }
    }
}

///////////////////////////////////////////////////////////////////////////////////////////////////////////////////////

#[cfg(test)]
mod tests {
    use super::*;

    const BACKGROUND: Color = Color {
        r: 22,
        g: 26,
        b: 34,
        a: 255,
    };
    const SEPARATOR: Color = Color {
        r: 90,
        g: 96,
        b: 110,
        a: 255,
    };
    const COLORS: [Color; 3] = [
        Color {
            r: 80,
            g: 220,
            b: 130,
            a: 255,
        },
        Color {
            r: 240,
            g: 180,
            b: 70,
            a: 255,
        },
        Color {
            r: 90,
            g: 170,
            b: 250,
            a: 255,
        },
    ];

    #[test]
    #[ignore = "embedded flowi capture must run serially"]
    fn capture_scope_display_matches_blessed_png() {
        let first: Vec<f32> = (0..320)
            .map(|index| ((index as f32 * 0.19).sin() * 1.2).clamp(-1.0, 1.0))
            .collect();
        let second: Vec<f32> = (0..320)
            .map(|index| ((index % 48) as f32 / 24.0) - 1.0)
            .collect();
        let third: Vec<f32> = (0..320)
            .map(|index| if index % 64 < 32 { 0.65 } else { -0.65 })
            .collect();
        let channels: [&[f32]; 3] = [&first, &second, &third];
        let mut display = ScopeDisplay::default();

        let frame = crate::pixel_oracle::capture(240, 112, || {
            ScopeDisplay::draw_channels(
                8.0, 8.0, 224.0, 42.0, &channels, &COLORS, BACKGROUND, SEPARATOR, 3.0,
            );
            display.draw_channels_oscilloscope(
                8.0, 62.0, 224.0, 42.0, &channels, &COLORS, BACKGROUND, SEPARATOR, 3.0,
            );
        });
        crate::pixel_oracle::assert_png("scope_display", &frame);
    }

    #[test]
    fn trigger_tracks_crossings_and_centers_silence() {
        let mut display = ScopeDisplay::default();
        let silence = [0.0; 320];
        assert_eq!(display.find_trigger(&silence, 0), 160);

        let waveform: Vec<f32> = (0..320)
            .map(|index| {
                let position = index as f32;
                (position * 0.13).sin() + (position * 0.031).sin() * 0.35
            })
            .collect();
        let first_trigger = display.find_trigger(&waveform, 0);
        assert!(display.triggers[0].initialized);

        let shifted: Vec<f32> = (0..320)
            .map(|index| {
                let position = index as f32 - 9.0;
                (position * 0.13).sin() + (position * 0.031).sin() * 0.35
            })
            .collect();
        assert_eq!(display.find_trigger(&shifted, 0), first_trigger + 9);
        assert_eq!(display.find_trigger(&waveform, MAX_CHANNELS), 0);
    }

    #[test]
    fn trigger_preserves_short_buffer_branches() {
        let mut display = ScopeDisplay::default();
        let too_short = [1.0; 15];
        assert_eq!(display.find_trigger(&too_short, 0), 0);

        let mut single_crossing = [1.0; 16];
        single_crossing[8..].fill(-1.0);
        assert_eq!(display.find_trigger(&single_crossing, 0), 8);
        assert!(!display.triggers[0].initialized);

        let rising: Vec<f32> = (0..63).map(|index| index as f32).collect();
        assert_eq!(display.find_trigger(&rising, 0), rising.len() / 2);
        assert!(!display.triggers[0].initialized);
    }

    #[test]
    fn empty_inputs_do_not_draw_or_mutate_trigger_state() {
        let mut display = ScopeDisplay::default();
        ScopeDisplay::draw_channel(0.0, 0.0, 10.0, 10.0, &[], COLORS[0]);
        display.draw_channel_oscilloscope(0.0, 0.0, 10.0, 10.0, &[], COLORS[0], 0);
        display.draw_channel_oscilloscope(0.0, 0.0, 10.0, 10.0, &[0.5; 4], COLORS[0], 0);
        assert!(!display.triggers[0].initialized);
    }
}
