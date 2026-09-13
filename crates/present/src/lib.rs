//! The presentation core every Retrovert UI shares.
//!
//! A renderer, whether the Quickshell shell on Omarchy or a flowi window, never reads
//! engine state directly. It reads the geometry this crate produces: a scope is a
//! polyline of normalised points, not a slice of samples. Every type here is plain data
//! with fixed capacity chosen at construction, so the steady-state visualization cadence
//! of the runtime contract holds: no allocation, no locks, bounded work per call.

use retrovert_host::ffi::playback::{RVColumnKind, RVPatternCell};
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

/// What a pattern cell holds, so a renderer can pick a colour without parsing the text.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u8)]
pub enum CellClass {
    /// Nothing in this cell: the decoder's placeholder text, dots or blanks.
    #[default]
    Empty = 0,
    /// A pitch.
    Note = 1,
    /// A note column entry that is not a pitch: note off, note cut, fade.
    NoteOff = 2,
    /// Instrument or sample number.
    Instrument = 3,
    /// Volume column.
    Volume = 4,
    /// Effect command.
    Effect = 5,
    /// Effect parameter.
    Param = 6,
    /// A plugin-specific column.
    Custom = 7,
}

impl CellClass {
    /// The class a column of `kind` gives to the text `text`.
    fn of(kind: RVColumnKind, text: &[u8]) -> Self {
        let blank = text
            .iter()
            .all(|&b| b == 0 || b == b'.' || b == b' ' || b == b'-');
        if blank {
            return Self::Empty;
        }
        match kind {
            RVColumnKind::Note => match text[0] {
                b'=' | b'^' | b'~' | b'#' => Self::NoteOff,
                _ => Self::Note,
            },
            RVColumnKind::Instrument => Self::Instrument,
            RVColumnKind::Volume => Self::Volume,
            RVColumnKind::Effect => Self::Effect,
            RVColumnKind::Param => Self::Param,
            RVColumnKind::Custom => Self::Custom,
        }
    }
}

/// One cell of the pattern grid as a renderer reads it: a class and the decoder's text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub struct GridCell {
    /// One of [`CellClass`], as its discriminant.
    pub class: u8,
    /// UTF-8, NUL-padded, never longer than the column's character width.
    pub text: [u8; 16],
}

impl Default for GridCell {
    fn default() -> Self {
        Self {
            class: CellClass::Empty as u8,
            text: [0; 16],
        }
    }
}

/// One pattern column: how wide its text is and what it holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GridColumn {
    /// Rendered width in characters.
    pub width: u8,
    /// What the column holds.
    pub kind: RVColumnKind,
}

/// The pattern grid a renderer draws: the decoder's row window as cells, plus where the
/// cursor is. Fixed capacity from the layout at construction; refilled in place per frame.
///
/// The cells are copied only when the window moves to another pattern, since a pattern's
/// text does not change while it plays; [`PatternGrid::fill_from`] says when that happened
/// so a renderer can keep its glyphs otherwise. Cell order is row-major: row, then channel,
/// then column, the order the decoder writes.
#[derive(Debug)]
pub struct PatternGrid {
    columns: Box<[GridColumn]>,
    channels: usize,
    cells: Box<[GridCell]>,
    rows: usize,
    window_lo: u32,
    cursor: u32,
    /// The pattern and window the cells were copied from, if any.
    key: Option<(u32, u32, u32)>,
}

impl PatternGrid {
    /// Reserves `row_budget` rows of `channels` channels of `columns`. Setup cadence.
    #[must_use]
    pub fn new(channels: usize, columns: &[GridColumn], row_budget: usize) -> Self {
        Self {
            columns: columns.into(),
            channels,
            cells: vec![GridCell::default(); row_budget * channels * columns.len()]
                .into_boxed_slice(),
            rows: 0,
            window_lo: 0,
            cursor: 0,
            key: None,
        }
    }

    /// A grid with nothing in it, for a session with no pattern to show.
    #[must_use]
    pub fn empty() -> Self {
        Self::new(0, &[], 0)
    }

    /// Pattern channels across.
    #[must_use]
    pub fn channels(&self) -> usize {
        self.channels
    }

    /// The columns each channel shows, in order.
    #[must_use]
    pub fn columns(&self) -> &[GridColumn] {
        &self.columns
    }

    /// Rows the window holds.
    #[must_use]
    pub fn rows(&self) -> usize {
        self.rows
    }

    /// The first row of the window, in pattern rows.
    #[must_use]
    pub fn window_lo(&self) -> u32 {
        self.window_lo
    }

    /// One past the last row of the window.
    #[must_use]
    pub fn window_hi(&self) -> u32 {
        self.window_lo
            .saturating_add(u32::try_from(self.rows).unwrap_or(u32::MAX))
    }

    /// The playing row, in pattern rows.
    #[must_use]
    pub fn cursor(&self) -> u32 {
        self.cursor
    }

    /// Every populated cell, row-major.
    #[must_use]
    pub fn cells(&self) -> &[GridCell] {
        &self.cells[..self.rows * self.channels * self.columns.len()]
    }

    /// The cells of pattern row `row`, or `None` outside the window.
    #[must_use]
    pub fn row(&self, row: u32) -> Option<&[GridCell]> {
        let index = usize::try_from(row.checked_sub(self.window_lo)?).ok()?;
        if index >= self.rows {
            return None;
        }
        let stride = self.channels * self.columns.len();
        Some(&self.cells[index * stride..(index + 1) * stride])
    }

    /// How many leading columns fit in `chars` characters when a space separates them:
    /// a narrow channel shows note and instrument, a wide one every column. At least one
    /// column is always shown.
    #[must_use]
    pub fn columns_fitting(&self, chars: usize) -> usize {
        let mut used = 0;
        let mut fit = 0;
        for column in &*self.columns {
            let need = usize::from(column.width) + usize::from(fit > 0);
            if fit > 0 && used + need > chars {
                break;
            }
            used += need;
            fit += 1;
        }
        fit
    }

    /// Takes the cursor and, when the window moved to another pattern, the cells from a
    /// snapshot. Returns whether the cells changed. A snapshot without a position clears
    /// the grid.
    pub fn fill_from(&mut self, snapshot: &VizSnapshot) -> bool {
        let Some(position) = snapshot.position else {
            let had = self.rows > 0;
            self.rows = 0;
            self.key = None;
            return had;
        };
        self.cursor = position.row;
        let key = (position.pattern, position.window_lo, position.window_hi);
        if self.key == Some(key) {
            return false;
        }
        self.key = Some(key);
        self.load(position.window_lo, position.row, snapshot.cells());
        true
    }

    /// Loads a window by hand: what [`PatternGrid::fill_from`] does with a snapshot's
    /// position and cells, for callers and tests that have only the pieces.
    pub fn load(&mut self, window_lo: u32, cursor: u32, cells: &[RVPatternCell]) {
        self.cursor = cursor;
        self.window_lo = window_lo;
        let stride = self.channels * self.columns.len();
        let rows = if stride == 0 {
            0
        } else {
            (cells.len() / stride).min(self.cells.len() / stride)
        };
        self.rows = rows;
        let columns = self.columns.len();
        for (i, (dst, src)) in self.cells[..rows * stride]
            .iter_mut()
            .zip(cells)
            .enumerate()
        {
            let kind = self.columns[i % columns].kind;
            *dst = GridCell {
                class: CellClass::of(kind, &src.text) as u8,
                text: src.text,
            };
        }
    }

    /// Copies another grid's window in, truncating to this capacity. Steady-state: a memcpy
    /// and nothing else, which is how a worker hands a pattern to a renderer. The other grid
    /// must have been built from the same layout.
    pub fn copy_from(&mut self, other: &PatternGrid) {
        let src = other.cells();
        let n = src.len().min(self.cells.len());
        self.cells[..n].copy_from_slice(&src[..n]);
        let stride = self.channels * self.columns.len();
        self.rows = if stride == 0 { 0 } else { n / stride };
        self.window_lo = other.window_lo;
        self.cursor = other.cursor;
        self.key = other.key;
    }

    /// Writes rows `lo..hi` into `out`, one full row of cells per pattern row, blank for rows
    /// outside the window, so a renderer always gets the block it asked for. Returns how
    /// many cells were written, which is at most `out.len()` rounded down to whole rows.
    pub fn copy_rows(&self, lo: u32, hi: u32, out: &mut [GridCell]) -> usize {
        let stride = self.channels * self.columns.len();
        if stride == 0 || hi <= lo {
            return 0;
        }
        let rows = usize::try_from(hi - lo)
            .unwrap_or(usize::MAX)
            .min(out.len() / stride);
        for (i, chunk) in out[..rows * stride].chunks_exact_mut(stride).enumerate() {
            let row = lo.saturating_add(u32::try_from(i).unwrap_or(u32::MAX));
            match self.row(row) {
                Some(src) => chunk.copy_from_slice(src),
                None => chunk.fill(GridCell::default()),
            }
        }
        rows * stride
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
    use retrovert_host::ffi::playback::RVPatternCell;

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

    fn cell(text: &str) -> RVPatternCell {
        let mut out = RVPatternCell {
            raw: 0,
            text: [0; 16],
        };
        out.text[..text.len()].copy_from_slice(text.as_bytes());
        out
    }

    fn grid() -> PatternGrid {
        PatternGrid::new(
            2,
            &[
                GridColumn {
                    width: 3,
                    kind: RVColumnKind::Note,
                },
                GridColumn {
                    width: 2,
                    kind: RVColumnKind::Instrument,
                },
                GridColumn {
                    width: 3,
                    kind: RVColumnKind::Volume,
                },
            ],
            4,
        )
    }

    #[test]
    fn classes_follow_the_column_and_the_text() {
        assert_eq!(CellClass::of(RVColumnKind::Note, b"C-4"), CellClass::Note);
        assert_eq!(
            CellClass::of(RVColumnKind::Note, b"==="),
            CellClass::NoteOff
        );
        assert_eq!(
            CellClass::of(RVColumnKind::Note, b"^^^"),
            CellClass::NoteOff
        );
        assert_eq!(CellClass::of(RVColumnKind::Note, b"..."), CellClass::Empty);
        assert_eq!(
            CellClass::of(RVColumnKind::Instrument, b".."),
            CellClass::Empty
        );
        assert_eq!(
            CellClass::of(RVColumnKind::Instrument, b"01"),
            CellClass::Instrument
        );
        assert_eq!(
            CellClass::of(RVColumnKind::Effect, b"\0\0"),
            CellClass::Empty
        );
        assert_eq!(CellClass::of(RVColumnKind::Param, b"0A"), CellClass::Param);
    }

    #[test]
    fn columns_fitting_keeps_a_prefix_and_at_least_one() {
        let g = grid();
        assert_eq!(g.columns_fitting(0), 1);
        assert_eq!(g.columns_fitting(5), 1);
        assert_eq!(g.columns_fitting(6), 2);
        assert_eq!(g.columns_fitting(9), 2);
        assert_eq!(g.columns_fitting(10), 3);
        assert_eq!(g.columns_fitting(100), 3);
    }

    #[test]
    fn copy_rows_blanks_rows_outside_the_window() {
        let mut g = grid();
        // Two rows of two channels of three columns, window 4..6.
        let texts = [
            "C-4", "01", "v64", "...", "..", "...", "D#5", "02", "...", "===", "..", "...",
        ];
        let cells: Vec<RVPatternCell> = texts.iter().map(|t| cell(t)).collect();
        g.load(4, 7, &cells);
        assert_eq!(g.rows(), 2);
        assert_eq!(g.window_hi(), 6);
        let mut out = vec![GridCell::default(); 3 * 6];
        assert_eq!(g.copy_rows(3, 6, &mut out), 18);
        assert_eq!(out[0].class, CellClass::Empty as u8);
        assert_eq!(&out[6].text[..3], b"C-4");
        assert_eq!(out[6].class, CellClass::Note as u8);
        assert_eq!(out[8].class, CellClass::Volume as u8);
        assert_eq!(out[15].class, CellClass::NoteOff as u8);
        assert!(g.row(6).is_none());
        assert_eq!(g.copy_rows(0, 0, &mut out), 0);
    }

    #[test]
    fn copy_from_carries_the_window_and_cursor() {
        let mut a = grid();
        a.load(0, 3, &[cell("A-3"); 6]);
        let mut b = grid();
        b.copy_from(&a);
        assert_eq!(b.rows(), 1);
        assert_eq!(b.cursor(), 3);
        assert_eq!(b.cells(), a.cells());
        assert_eq!(
            PatternGrid::empty().copy_rows(0, 4, &mut [GridCell::default(); 4]),
            0
        );
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
