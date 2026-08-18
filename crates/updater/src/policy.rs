//! Policy values every transfer path agrees on.
//!
//! Each of these was written two or three times over in the code this crate
//! was copied from, and the copies had already started to disagree. One home
//! keeps them from drifting apart again.

/// The first retry's delay; each retry after it doubles.
const RETRY_BASE_DELAY_MS: i64 = 1000;

/// The largest shift [`retry_delay_ms`] applies, which caps the delay at just
/// under three weeks and keeps a runaway retry count from overflowing.
const MAX_RETRY_SHIFT: u32 = 30;

/// Whether a path is an HTTP(S) URL rather than something local.
///
/// Only `http://` and `https://` qualify — not `ftp://`, not a POSIX path, not
/// a Windows one.
#[must_use]
pub fn is_url(path: &str) -> bool {
    path.starts_with("http://") || path.starts_with("https://")
}

/// Fractional progress in `0.0..=1.0`, or `0.0` when the total is unknown.
#[must_use]
pub fn progress(bytes_done: i64, bytes_total: i64) -> f32 {
    if bytes_total <= 0 {
        return 0.0;
    }
    // A transfer big enough to lose `f32` mantissa bits loses them far below
    // one pixel of the progress bar this feeds.
    #[allow(clippy::cast_precision_loss)]
    let fraction = bytes_done as f32 / bytes_total as f32;
    fraction
}

/// The delay before the `retry_count`-th retry: 1s, 2s, 4s, and so on.
#[must_use]
pub fn retry_delay_ms(retry_count: u32) -> i64 {
    RETRY_BASE_DELAY_MS << retry_count.min(MAX_RETRY_SHIFT)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_http_and_https_are_urls() {
        assert!(is_url("http://example.com/file.zip"));
        assert!(is_url("https://example.com/file.zip"));
        assert!(!is_url("/home/user/file.zip"));
        assert!(!is_url("data/file.zip"));
        assert!(!is_url("C:\\Users\\file.zip"));
        assert!(!is_url("ftp://example.com/file.zip"));
    }

    #[test]
    fn progress_is_a_ratio_and_zero_when_the_total_is_unknown() {
        assert!((progress(50, 100) - 0.5).abs() < 1e-6);
        assert!((progress(0, -1) - 0.0).abs() < 1e-6);
        assert!((progress(10, 0) - 0.0).abs() < 1e-6);
        assert!((progress(100, 100) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn backoff_is_exponential() {
        assert_eq!(retry_delay_ms(0), 1000);
        assert_eq!(retry_delay_ms(1), 2000);
        assert_eq!(retry_delay_ms(2), 4000);
    }

    #[test]
    fn a_runaway_retry_count_saturates_rather_than_overflowing() {
        let capped = retry_delay_ms(MAX_RETRY_SHIFT);
        assert_eq!(retry_delay_ms(u32::MAX), capped);
        assert!(capped > 0);
    }
}
