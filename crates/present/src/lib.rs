//! The presentation core every Retrovert UI shares.
//!
//! A renderer, whether the Quickshell shell on Omarchy or a flowi window, never reads
//! engine state directly. It reads the geometry this crate produces: a scope is a
//! polyline of normalised points, not a slice of samples. Every type here is plain data
//! with fixed capacity chosen at construction, so the steady-state visualization cadence
//! of the runtime contract holds: no allocation, no locks, bounded work per call.

use retrovert_host::visualization::VizSnapshot;

/// One vertex of a scope polyline. `x` runs 0..=1 across the width, `y` is the sample
/// in -1..=1 with positive up, so a renderer maps it to its own pixel box.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
#[repr(C)]
pub struct ScopePoint {
    /// Horizontal position, 0 at the first sample and 1 at the last.
    pub x: f32,
    /// Sample value, clamped to -1..=1.
    pub y: f32,
}

/// A fixed-capacity polyline for one oscilloscope channel, refilled in place per frame.
///
/// When a snapshot holds more samples than the trace has points, samples are bucketed
/// and each bucket contributes the sample of largest magnitude, which keeps transients
/// visible instead of aliasing them away.
#[derive(Debug)]
pub struct ScopeTrace {
    points: Box<[ScopePoint]>,
    len: usize,
}

impl ScopeTrace {
    /// Reserves `capacity` points. Setup cadence: this is the one allocation.
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        Self {
            points: vec![ScopePoint::default(); capacity.max(2)].into_boxed_slice(),
            len: 0,
        }
    }

    /// How many points a full trace holds.
    #[must_use]
    pub fn capacity(&self) -> usize {
        self.points.len()
    }

    /// The points populated by the last [`ScopeTrace::fill`].
    #[must_use]
    pub fn points(&self) -> &[ScopePoint] {
        &self.points[..self.len]
    }

    /// Refills the trace from raw samples. Empty input yields a flat line of two points
    /// so a renderer never has to special-case a silent channel.
    pub fn fill(&mut self, samples: &[f32]) {
        let cap = self.points.len();
        if samples.is_empty() {
            self.points[0] = ScopePoint { x: 0.0, y: 0.0 };
            self.points[1] = ScopePoint { x: 1.0, y: 0.0 };
            self.len = 2;
            return;
        }
        let n = samples.len().min(cap);
        let last = n.saturating_sub(1).max(1);
        // Precision loss is irrelevant here: these are pixel-scale ratios.
        #[allow(clippy::cast_precision_loss)]
        let step = 1.0 / last as f32;
        for (i, point) in self.points[..n].iter_mut().enumerate() {
            let lo = i * samples.len() / n;
            let hi = ((i + 1) * samples.len() / n).max(lo + 1);
            let y = samples[lo..hi].iter().copied().fold(0.0_f32, |best, s| {
                if s.abs() > best.abs() {
                    s
                } else {
                    best
                }
            });
            #[allow(clippy::cast_precision_loss)]
            let x = i as f32 * step;
            *point = ScopePoint {
                x,
                y: y.clamp(-1.0, 1.0),
            };
        }
        self.len = n;
    }

    /// Copies another trace's populated points in, truncating to this capacity. Steady-state:
    /// a memcpy and nothing else, which is how a worker hands a frame to a renderer.
    pub fn copy_from(&mut self, other: &ScopeTrace) {
        let src = other.points();
        let n = src.len().min(self.points.len());
        self.points[..n].copy_from_slice(&src[..n]);
        self.len = n;
    }

    /// Refills from one channel of a snapshot; a channel the snapshot lacks flattens.
    pub fn fill_from(&mut self, snapshot: &VizSnapshot, channel: usize) {
        self.fill(snapshot.scope(channel).unwrap_or(&[]));
    }
}

/// Fixed-capacity VU levels with a fall rate, refilled in place per frame.
///
/// A decoder reports the instantaneous level of each channel; a meter that showed it raw
/// would flicker at the capture rate. Each level jumps up to a louder reading at once and
/// falls from a quieter one by `fall` per tick, so a hit stays visible for a few frames.
#[derive(Debug)]
pub struct VuMeter {
    levels: Box<[f32]>,
    len: usize,
    fall: f32,
}

impl VuMeter {
    /// Reserves `capacity` channels falling by `fall` (of full scale) per tick. Setup cadence.
    #[must_use]
    pub fn new(capacity: usize, fall: f32) -> Self {
        Self {
            levels: vec![0.0; capacity.max(1)].into_boxed_slice(),
            len: 0,
            fall: fall.max(0.0),
        }
    }

    /// The levels populated by the last [`VuMeter::tick`], one per channel in 0..=1.
    #[must_use]
    pub fn levels(&self) -> &[f32] {
        &self.levels[..self.len]
    }

    /// Feeds one capture's readings: each channel rises to its reading at once and otherwise
    /// falls by the meter's rate. Channels past the capacity are dropped; a shorter reading
    /// shrinks the meter to its length.
    pub fn tick(&mut self, readings: &[f32]) {
        let n = readings.len().min(self.levels.len());
        for (level, &reading) in self.levels[..n].iter_mut().zip(readings) {
            let reading = reading.clamp(0.0, 1.0);
            *level = if reading >= *level {
                reading
            } else {
                (*level - self.fall).max(reading)
            };
        }
        self.len = n;
    }

    /// Feeds one capture. A decoder that reports VU levels is believed; one that reports only
    /// scopes is metered by each channel's peak over the capture, so every song has a meter.
    pub fn tick_from(&mut self, snapshot: &VizSnapshot) {
        if !snapshot.vu().is_empty() {
            self.tick(snapshot.vu());
            return;
        }
        let channels = snapshot.scope_counts().len().min(self.levels.len());
        for channel in 0..channels {
            let reading = snapshot
                .scope(channel)
                .unwrap_or(&[])
                .iter()
                .fold(0.0_f32, |peak, s| peak.max(s.abs()))
                .clamp(0.0, 1.0);
            let level = &mut self.levels[channel];
            *level = if reading >= *level {
                reading
            } else {
                (*level - self.fall).max(reading)
            };
        }
        self.len = channels;
    }

    /// Drops every level to silence, as when a song is unmounted.
    pub fn clear(&mut self) {
        self.levels.fill(0.0);
        self.len = 0;
    }
}

#[cfg(test)]
// Exact comparison is the point: these are values the trace copies or clamps, not computes.
#[allow(clippy::float_cmp)]
mod tests {
    use super::*;

    #[test]
    fn empty_input_is_a_flat_line() {
        let mut trace = ScopeTrace::new(8);
        trace.fill(&[]);
        assert_eq!(
            trace.points(),
            &[ScopePoint { x: 0.0, y: 0.0 }, ScopePoint { x: 1.0, y: 0.0 }]
        );
    }

    #[test]
    fn short_input_spans_the_width() {
        let mut trace = ScopeTrace::new(8);
        trace.fill(&[0.5, -0.5, 0.25]);
        let pts = trace.points();
        assert_eq!(pts.len(), 3);
        assert_eq!(pts[0].x, 0.0);
        assert_eq!(pts[2].x, 1.0);
        assert_eq!(pts[1].y, -0.5);
    }

    #[test]
    fn long_input_keeps_the_peak_of_each_bucket() {
        let mut trace = ScopeTrace::new(4);
        let samples = [0.1, 0.9, -0.2, 0.0, 0.3, -0.8, 0.0, 0.05];
        trace.fill(&samples);
        let ys: Vec<f32> = trace.points().iter().map(|p| p.y).collect();
        assert_eq!(ys, vec![0.9, -0.2, -0.8, 0.05]);
    }

    #[test]
    fn values_are_clamped() {
        let mut trace = ScopeTrace::new(4);
        trace.fill(&[3.0, -3.0]);
        assert_eq!(trace.points()[0].y, 1.0);
        assert_eq!(trace.points()[1].y, -1.0);
    }

    #[test]
    fn copy_from_truncates_to_capacity() {
        let mut big = ScopeTrace::new(8);
        big.fill(&[0.1; 8]);
        let mut small = ScopeTrace::new(4);
        small.copy_from(&big);
        assert_eq!(small.points().len(), 4);
        assert_eq!(small.points()[3].y, 0.1);
    }

    #[test]
    fn refill_never_grows() {
        let mut trace = ScopeTrace::new(16);
        let before = trace.points.as_ptr();
        trace.fill(&[0.0; 100]);
        trace.fill(&[0.0; 3]);
        assert_eq!(trace.points.as_ptr(), before);
        assert_eq!(trace.capacity(), 16);
    }

    #[test]
    fn vu_rises_at_once_and_falls_by_the_rate() {
        let mut meter = VuMeter::new(4, 0.25);
        meter.tick(&[1.0, 0.5]);
        assert_eq!(meter.levels(), &[1.0, 0.5]);
        meter.tick(&[0.0, 0.0]);
        assert_eq!(meter.levels(), &[0.75, 0.25]);
        meter.tick(&[0.0, 0.9]);
        assert_eq!(meter.levels(), &[0.5, 0.9]);
    }

    #[test]
    fn vu_clamps_readings_and_drops_channels_past_capacity() {
        let mut meter = VuMeter::new(2, 0.1);
        meter.tick(&[3.0, -1.0, 0.5]);
        assert_eq!(meter.levels(), &[1.0, 0.0]);
        meter.clear();
        assert!(meter.levels().is_empty());
    }
}
