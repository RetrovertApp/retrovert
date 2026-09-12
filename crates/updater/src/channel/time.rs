//! Where a check's verification time comes from.
//!
//! Never the local clock alone. An attacker who can move it can make expired
//! metadata look current, so the time comes from the network and the check
//! runs against that or does not run at all. There is no fallback: a check
//! with no network time reports itself skipped and leaves trust state alone.
//!
//! The network's word is clamped, not taken whole: whoever serves the `Date`
//! header could otherwise move verification time backwards and present a
//! stale — still signed, since expired — chain as current. A check therefore
//! runs at the later of the reported time and the local clock, and the trust
//! state additionally refuses a time earlier than one already verified
//! against (see [`super::trust::Floor`]).

use std::sync::Arc;

use jiff::Timestamp;
use retrovert_tuf::RoleName;
use sigstore_tuf::UpdaterConfig;

use super::error::{Error, Result};
use super::source;
use crate::transport::Transport;

/// A verification time obtained from the network.
///
/// Constructing one is the claim that the instant did not come from the local
/// clock; nothing verifies against a bare [`Timestamp`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NetworkTime(Timestamp);

impl NetworkTime {
    /// The instant the network reported.
    #[must_use]
    pub fn new(at: Timestamp) -> Self {
        Self(at)
    }

    /// The instant itself.
    #[must_use]
    pub fn timestamp(&self) -> Timestamp {
        self.0
    }
}

/// A source of verification time.
pub trait Clock: Send + Sync {
    /// The current time as the network reports it, or `None` when the network
    /// could not be asked. `None` skips the check; it never means "use the
    /// local clock".
    fn network_time(&self) -> Option<NetworkTime>;
}

/// The `Date` header of the metadata host serving a channel.
///
/// The header is worth something only because TLS authenticates the host that
/// sent it. A plaintext base URL is refused at construction, and the request
/// itself goes out over a transport that will not speak plaintext to a
/// redirect target either — otherwise a 302 to `http://` would hand back a
/// time nobody vouched for.
pub struct HostDate {
    transport: Arc<Transport>,
    base: String,
}

impl std::fmt::Debug for HostDate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HostDate")
            .field("base", &self.base)
            .finish_non_exhaustive()
    }
}

impl HostDate {
    /// Take time from the host serving the channel at `base_url`.
    pub fn new(transport: Arc<Transport>, base_url: &str) -> Result<Self> {
        if !base_url.starts_with("https://") {
            return Err(Error::Insecure(base_url.to_string()));
        }
        Ok(Self {
            transport,
            base: source::base_of(base_url),
        })
    }
}

impl Clock for HostDate {
    fn network_time(&self) -> Option<NetworkTime> {
        // The channel's own entry point, cache-busted the same way the refresh
        // busts it. Re-reading a file the refresh is about to read anyway costs
        // less than reaching for a second endpoint whose clock nobody pinned.
        let name = RoleName::Timestamp.file_name();
        let url = format!("{}{name}{}", self.base, source::cache_key(&name));
        let limit =
            usize::try_from(UpdaterConfig::default().timestamp_max_length).unwrap_or(usize::MAX);

        let date = self
            .transport
            .get_bounded_over_tls(&url, limit)
            .ok()?
            .date?;
        let reported = parse_http_date(&date)?;
        Some(NetworkTime::new(verification_time(
            reported,
            Timestamp::now(),
        )))
    }
}

/// Parse the `Date` header's RFC 9110 IMF-fixdate form.
fn parse_http_date(value: &str) -> Option<Timestamp> {
    jiff::fmt::rfc2822::parse(value)
        .ok()
        .map(|zoned| zoned.timestamp())
}

/// The instant a check verifies against: the host's word, but never earlier
/// than this machine's own clock says it is.
///
/// Taking the later of the two means backdating a check needs the local clock
/// moved as well as the `Date` header. A local clock running fast can only
/// fail closed — valid metadata reads as expired until the clock is fixed —
/// never accept anything stale.
fn verification_time(reported: Timestamp, local: Timestamp) -> Timestamp {
    reported.max(local)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_imf_fixdate_header_parses_to_its_instant() {
        assert_eq!(
            parse_http_date("Sun, 06 Nov 1994 08:49:37 GMT"),
            Some("1994-11-06T08:49:37Z".parse().unwrap())
        );
    }

    #[test]
    fn a_header_that_is_not_a_date_yields_no_time() {
        for bad in [
            "",
            "now",
            "2026-08-15T12:00:00Z",
            "Sun, 32 Nov 1994 08:49:37 GMT",
        ] {
            assert_eq!(parse_http_date(bad), None, "{bad:?} must not parse");
        }
    }

    #[test]
    fn a_reported_time_is_never_earlier_than_the_local_clock() {
        let earlier: Timestamp = "2026-08-15T12:00:00Z".parse().unwrap();
        let later: Timestamp = "2026-08-15T13:00:00Z".parse().unwrap();

        // A backdated Date is lifted to the local clock; an honest Date on a
        // slow local clock is taken as reported.
        assert_eq!(verification_time(earlier, later), later);
        assert_eq!(verification_time(later, earlier), later);
        assert_eq!(verification_time(later, later), later);
    }

    #[test]
    fn a_plaintext_host_is_refused_as_a_time_source() {
        let transport = Arc::new(Transport::new("/cache"));
        let err = HostDate::new(Arc::clone(&transport), "http://host/dev").unwrap_err();
        assert!(matches!(err, Error::Insecure(url) if url == "http://host/dev"));
        assert!(HostDate::new(transport, "https://host/dev").is_ok());
    }

    #[test]
    fn a_host_that_cannot_be_reached_yields_no_time_at_all() {
        // Port 1 on loopback refuses instantly, so this stays offline. The
        // answer that matters is that a failed request produces `None` and not
        // the local clock; the success branch needs a real TLS host and belongs
        // to the opt-in live test.
        let transport = Arc::new(Transport::new("/cache"));
        let time = HostDate::new(transport, "https://127.0.0.1:1/channel").unwrap();
        assert_eq!(time.network_time(), None);
    }
}
