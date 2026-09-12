//! The channel's fixed TUF policy: spec version and role expiry lifetimes.
//!
//! Timestamp is the shortest-lived role, so a channel nobody re-signs stops
//! verifying within a quarter; snapshot and targets outlive it so that a
//! monthly re-sign job has months of slack; root is long-lived because
//! renewing it needs the offline key and a frontend release.

use jiff::{Span, Timestamp, tz::TimeZone};

use crate::error::Result;
use crate::metadata::RoleName;

/// The TUF specification version this publisher writes.
pub const SPEC_VERSION: &str = "1.0.33";

/// Days a `timestamp` role stays valid.
pub const TIMESTAMP_EXPIRY_DAYS: i64 = 90;

/// Days a `snapshot` or `targets` role stays valid.
pub const SNAPSHOT_TARGETS_EXPIRY_DAYS: i64 = 365;

/// Years a `root` role stays valid.
pub const ROOT_EXPIRY_YEARS: i64 = 10;

/// The lifetime granted to `role` at signing time.
#[must_use]
pub fn expiry_span(role: RoleName) -> Span {
    match role {
        RoleName::Root => Span::new().years(ROOT_EXPIRY_YEARS),
        RoleName::Targets | RoleName::Snapshot => Span::new().days(SNAPSHOT_TARGETS_EXPIRY_DAYS),
        RoleName::Timestamp => Span::new().days(TIMESTAMP_EXPIRY_DAYS),
    }
}

/// When `role` signed at `now` expires.
///
/// Arithmetic runs in UTC, where a calendar day is exactly 24 hours, so the
/// result is independent of the machine's local zone.
pub fn expires_at(role: RoleName, now: Timestamp) -> Result<Timestamp> {
    Ok(now
        .to_zoned(TimeZone::UTC)
        .checked_add(expiry_span(role))?
        .timestamp())
}

/// Render a timestamp as the whole-second UTC RFC 3339 form TUF metadata uses.
#[must_use]
pub fn format_expires(at: Timestamp) -> String {
    at.strftime("%Y-%m-%dT%H:%M:%SZ").to_string()
}

/// The `expires` field for `role` signed at `now`.
pub fn expires(role: RoleName, now: Timestamp) -> Result<String> {
    Ok(format_expires(expires_at(role, now)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn now() -> Timestamp {
        "2026-08-15T12:00:00Z".parse().unwrap()
    }

    #[test]
    fn expiries_match_the_decided_policy() {
        assert_eq!(
            expires(RoleName::Timestamp, now()).unwrap(),
            "2026-11-13T12:00:00Z"
        );
        assert_eq!(
            expires(RoleName::Snapshot, now()).unwrap(),
            "2027-08-15T12:00:00Z"
        );
        assert_eq!(
            expires(RoleName::Targets, now()).unwrap(),
            "2027-08-15T12:00:00Z"
        );
        assert_eq!(
            expires(RoleName::Root, now()).unwrap(),
            "2036-08-15T12:00:00Z"
        );
    }

    #[test]
    fn sub_second_precision_is_dropped() {
        let now: Timestamp = "2026-08-15T12:00:00.987654321Z".parse().unwrap();
        assert_eq!(
            expires(RoleName::Timestamp, now).unwrap(),
            "2026-11-13T12:00:00Z"
        );
    }
}
