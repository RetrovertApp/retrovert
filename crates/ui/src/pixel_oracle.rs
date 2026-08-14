use flowi::{grow, layout, Application, EmbeddedSettings};
use std::path::{Path, PathBuf};

const BYTES_PER_PIXEL: usize = 3;
const SETTLE_FRAMES: usize = 3;
const CLEAR_RGB: [u8; BYTES_PER_PIXEL] = [14, 14, 16];
const BLESS_ENV: &str = "REPLAY_BLESS_PIXEL_ORACLES";

pub struct RgbFrame {
    width: u32,
    height: u32,
    pixels: Vec<u8>,
}

impl RgbFrame {
    pub fn new(width: u32, height: u32, pixels: Vec<u8>) -> RgbFrame {
        assert_eq!(
            pixels.len(),
            width as usize * height as usize * BYTES_PER_PIXEL
        );
        RgbFrame {
            width,
            height,
            pixels,
        }
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    pub fn pixels(&self) -> &[u8] {
        &self.pixels
    }
}

pub fn capture(width: u32, height: u32, mut draw: impl FnMut()) -> RgbFrame {
    capture_frames(width, height, SETTLE_FRAMES, &mut draw)
}

pub fn capture_frames(
    width: u32,
    height: u32,
    settle_frames: usize,
    mut draw: impl FnMut(),
) -> RgbFrame {
    let Some(mut app) = Application::embedded(EmbeddedSettings::new(width, height)) else {
        panic!("embedded application failed to start");
    };
    for _ in 0..settle_frames {
        app.tick_embedded(1.0 / 60.0, || {
            layout! { width: grow!(), height: grow!() {
                draw();
            }}
        });
    }

    let Some(readback) = app.pixels() else {
        panic!("software backend returned no pixels");
    };
    let captured_width = readback.width();
    let captured_height = readback.height();
    let row_bytes = captured_width as usize * BYTES_PER_PIXEL;
    let mut pixels = Vec::with_capacity(row_bytes * captured_height as usize);
    for row in readback.rows() {
        pixels.extend_from_slice(&row[..row_bytes]);
    }

    let frame = RgbFrame::new(captured_width, captured_height, pixels);
    assert_eq!((frame.width, frame.height), (width, height));
    assert!(ink_count(&frame) > 0, "capture contains no rendered ink");
    frame
}

pub fn assert_png(name: &str, actual: &RgbFrame) {
    let baseline = baseline_path(name);
    let diff_path = output_path(name, "diff");
    match compare_png(
        actual,
        &baseline,
        &diff_path,
        std::env::var_os(BLESS_ENV).is_some(),
    ) {
        Ok(()) => {}
        Err(PixelOracleError::Missing) => panic!(
            "pixel oracle {} is missing; inspect the render and rerun with {BLESS_ENV}=1",
            baseline.display()
        ),
        Err(PixelOracleError::Dimensions) => panic!("pixel oracle dimensions changed"),
        Err(PixelOracleError::Pixels(mismatches)) => panic!(
            "pixel oracle differs at {mismatches} pixels; diff: {}",
            diff_path.display()
        ),
    }
}

#[derive(Debug, PartialEq)]
enum PixelOracleError {
    Missing,
    Dimensions,
    Pixels(usize),
}

fn compare_png(
    actual: &RgbFrame,
    baseline: &Path,
    diff_path: &Path,
    bless: bool,
) -> Result<(), PixelOracleError> {
    if bless {
        write_png(baseline, actual);
        return Ok(());
    }
    let Some(expected) = read_png(baseline) else {
        return Err(PixelOracleError::Missing);
    };
    if (actual.width, actual.height) != (expected.width, expected.height) {
        return Err(PixelOracleError::Dimensions);
    }
    let (mismatches, diff) = diff(actual, &expected);
    if mismatches == 0 {
        return Ok(());
    }
    write_png(diff_path, &diff);
    Err(PixelOracleError::Pixels(mismatches))
}

fn ink_count(frame: &RgbFrame) -> usize {
    frame
        .pixels
        .chunks_exact(BYTES_PER_PIXEL)
        .filter(|pixel| *pixel != CLEAR_RGB)
        .count()
}

fn diff(actual: &RgbFrame, expected: &RgbFrame) -> (usize, RgbFrame) {
    assert_eq!(
        (actual.width, actual.height),
        (expected.width, expected.height)
    );
    let mut mismatches = 0;
    let mut pixels = Vec::with_capacity(actual.pixels.len());
    for (got, want) in actual
        .pixels
        .chunks_exact(BYTES_PER_PIXEL)
        .zip(expected.pixels.chunks_exact(BYTES_PER_PIXEL))
    {
        if got == want {
            pixels.extend_from_slice(&[got[0] / 4, got[1] / 4, got[2] / 4]);
        } else {
            mismatches += 1;
            pixels.extend_from_slice(&[255, 0, 0]);
        }
    }
    (
        mismatches,
        RgbFrame::new(actual.width, actual.height, pixels),
    )
}

fn baseline_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/music-player/pixels")
        .join(format!("{name}.png"))
}

fn output_path(name: &str, suffix: &str) -> PathBuf {
    std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target"))
        .join("test-screenshots")
        .join(format!("{name}.{suffix}.png"))
}

fn write_png(path: &Path, frame: &RgbFrame) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .unwrap_or_else(|err| panic!("cannot create {}: {err}", parent.display()));
    }
    let file = std::fs::File::create(path)
        .unwrap_or_else(|err| panic!("cannot create {}: {err}", path.display()));
    let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), frame.width, frame.height);
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder
        .write_header()
        .unwrap_or_else(|err| panic!("cannot encode {}: {err}", path.display()));
    writer
        .write_image_data(&frame.pixels)
        .unwrap_or_else(|err| panic!("cannot write {}: {err}", path.display()));
}

fn read_png(path: &Path) -> Option<RgbFrame> {
    let file = std::fs::File::open(path).ok()?;
    let decoder = png::Decoder::new(std::io::BufReader::new(file));
    let mut reader = decoder
        .read_info()
        .unwrap_or_else(|err| panic!("cannot decode {}: {err}", path.display()));
    let mut pixels = vec![0; reader.output_buffer_size()];
    let info = reader
        .next_frame(&mut pixels)
        .unwrap_or_else(|err| panic!("cannot read {}: {err}", path.display()));
    assert_eq!(info.color_type, png::ColorType::Rgb);
    assert_eq!(info.bit_depth, png::BitDepth::Eight);
    pixels.truncate(info.buffer_size());
    Some(RgbFrame::new(info.width, info.height, pixels))
}

///////////////////////////////////////////////////////////////////////////////////////////////////////////////////////

#[cfg(test)]
mod tests {
    use super::*;
    use flowi::{Color, Painter};

    #[test]
    fn png_round_trip_and_diff_are_exact() {
        let _assert_api: fn(&str, &RgbFrame) = assert_png;
        let frame = RgbFrame::new(2, 1, vec![10, 20, 30, 40, 50, 60]);
        assert_eq!((frame.width(), frame.height()), (2, 1));
        let path = output_path("pixel_oracle_round_trip", "actual");
        write_png(&path, &frame);
        let Some(decoded) = read_png(&path) else {
            panic!("round-trip PNG was just written");
        };
        assert_eq!(decoded.pixels(), frame.pixels());

        let changed = RgbFrame::new(2, 1, vec![10, 20, 30, 41, 50, 60]);
        let (count, image) = diff(&frame, &changed);
        assert_eq!(count, 1);
        assert_eq!(image.pixels(), &[2, 5, 7, 255, 0, 0]);
    }

    #[test]
    fn compare_covers_bless_match_and_failures() {
        let baseline = output_path("pixel_oracle_compare", "baseline");
        let diff_path = output_path("pixel_oracle_compare", "diff");
        let expected = RgbFrame::new(2, 1, vec![10, 20, 30, 40, 50, 60]);

        assert_eq!(compare_png(&expected, &baseline, &diff_path, true), Ok(()));
        assert_eq!(compare_png(&expected, &baseline, &diff_path, false), Ok(()));

        let changed = RgbFrame::new(2, 1, vec![10, 20, 30, 41, 50, 60]);
        assert_eq!(
            compare_png(&changed, &baseline, &diff_path, false),
            Err(PixelOracleError::Pixels(1))
        );
        assert!(diff_path.is_file());

        let wrong_size = RgbFrame::new(1, 1, vec![10, 20, 30]);
        assert_eq!(
            compare_png(&wrong_size, &baseline, &diff_path, false),
            Err(PixelOracleError::Dimensions)
        );
        let missing = output_path("pixel_oracle_missing", "baseline");
        assert_eq!(
            compare_png(&expected, &missing, &diff_path, false),
            Err(PixelOracleError::Missing)
        );
    }

    #[test]
    fn assert_png_uses_the_locked_path_and_maps_errors() {
        let frame = RgbFrame::new(2, 1, vec![10, 20, 30, 40, 50, 60]);
        assert_png("pixel_oracle_harness", &frame);

        if std::env::var_os(BLESS_ENV).is_none() {
            let panic = std::panic::catch_unwind(|| assert_png("pixel_oracle_missing", &frame));
            assert!(panic.is_err());
        }
    }

    #[test]
    #[ignore = "embedded flowi capture must run serially"]
    fn capture_has_dimensions_and_ink() {
        let frame = capture(64, 48, || {
            Painter::rect(
                4.0,
                4.0,
                24.0,
                16.0,
                Color {
                    r: 220,
                    g: 40,
                    b: 80,
                    a: 255,
                },
            );
        });
        assert_eq!((frame.width(), frame.height()), (64, 48));
        assert!(ink_count(&frame) > 0);
    }
}
