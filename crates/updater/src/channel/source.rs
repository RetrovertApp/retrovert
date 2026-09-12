//! Reading a channel over HTTP: one base URL, one flat namespace.

use std::sync::Arc;

use jiff::Timestamp;
use retrovert_tuf::RoleName;
use sigstore_tuf::Error as TufError;
use sigstore_tuf::transport::{FetchFuture, Repository};

use crate::transport::{Error as TransportError, Transport};

/// A channel served over HTTPS from one flat namespace.
///
/// Release assets have no directories, so a channel's metadata and its target
/// sit side by side under a single base URL — which is exactly the shape TUF's
/// consistent-snapshot naming already guarantees is collision-free.
pub struct HttpSource {
    transport: Arc<Transport>,
    base: String,
}

impl HttpSource {
    /// Read the channel published at `base_url`.
    #[must_use]
    pub fn new(transport: Arc<Transport>, base_url: &str) -> Self {
        Self {
            transport,
            base: base_of(base_url),
        }
    }

    fn fetch(&self, name: &str, max_length: u64) -> sigstore_tuf::Result<Option<Vec<u8>>> {
        let name = flat_name(name)?;
        let url = format!("{}{name}{}", self.base, cache_key(name));
        let limit = usize::try_from(max_length).unwrap_or(usize::MAX);

        match self.transport.get_bounded(&url, limit) {
            Ok(response) => Ok(Some(response.body)),
            // The root chain walk reads a 404 as "no such rotation", which is
            // how it learns it has reached the newest root.
            Err(TransportError::Status { status: 404, .. }) => Ok(None),
            Err(e) => Err(TufError::Transport(e.to_string())),
        }
    }
}

impl Repository for HttpSource {
    fn fetch_metadata<'a>(&'a self, name: &'a str, max_length: u64) -> FetchFuture<'a> {
        let result = self.fetch(name, max_length);
        Box::pin(async move { result })
    }

    fn fetch_target<'a>(&'a self, path: &'a str, max_length: u64) -> FetchFuture<'a> {
        let result = self.fetch(path, max_length);
        Box::pin(async move { result })
    }
}

/// A base URL ending in exactly one separator, so joining a name onto it cannot
/// double up or run the two together.
pub(super) fn base_of(base_url: &str) -> String {
    format!("{}/", base_url.trim_end_matches('/'))
}

/// A query that makes a mutable name's URL one no cache has seen, or nothing at
/// all for a name that cannot change.
///
/// Consistent snapshots put a version or a digest in almost every name, and
/// those are worth caching — the bytes behind them never move. The handful of
/// names that *are* replaced are a different matter: GitHub's release-download
/// endpoint serves them from an edge cache for minutes at a time and ignores a
/// `no-cache` request header, which turns "what is this channel serving now"
/// into "what was it serving a while ago". A per-request key is the only thing
/// that reliably gets an answer about the present.
///
/// The local clock supplies the key. It is a nonce, not a time: nothing is
/// verified against it.
pub(super) fn cache_key(name: &str) -> String {
    let mutable = name == RoleName::Timestamp.file_name() || name == RoleName::Root.file_name();
    if mutable {
        format!("?t={}", Timestamp::now().as_nanosecond())
    } else {
        String::new()
    }
}

/// Check that `name` addresses one asset under the base URL and nothing else.
///
/// A flat namespace has no way to express a nested target path, and a name that
/// reached outside it would be resolving something the base URL never promised.
fn flat_name(name: &str) -> sigstore_tuf::Result<&str> {
    let rejected = name.is_empty()
        || name.contains(['/', '\\', '?', '#'])
        || name.starts_with('.') && name.trim_start_matches('.').is_empty();
    if rejected {
        return Err(TufError::Transport(format!(
            "{name:?} does not name an asset on a flat channel"
        )));
    }
    Ok(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_base_url_ends_in_exactly_one_separator() {
        for given in [
            "https://host/download/dev/channel-metadata",
            "https://host/download/dev/channel-metadata/",
            "https://host/download/dev/channel-metadata///",
        ] {
            assert_eq!(
                base_of(given),
                "https://host/download/dev/channel-metadata/"
            );
        }
    }

    #[test]
    fn only_the_names_that_get_replaced_defeat_the_cache() {
        for immutable in ["3.snapshot.json", "2.root.json", "ab12.manifest.json"] {
            assert_eq!(cache_key(immutable), "", "{immutable} never changes");
        }
        for mutable in ["timestamp.json", "root.json"] {
            assert!(cache_key(mutable).starts_with("?t="), "{mutable}");
        }
        assert_ne!(
            cache_key("timestamp.json"),
            cache_key("timestamp.json"),
            "two polls must not share a cache entry"
        );
    }

    #[test]
    fn only_flat_asset_names_are_fetched() {
        for name in ["timestamp.json", "3.snapshot.json", "ab12.manifest.json"] {
            assert_eq!(flat_name(name).unwrap(), name);
        }
        for name in ["", ".", "..", "nested/manifest.json", "a\\b", "a?b", "a#b"] {
            assert!(flat_name(name).is_err(), "{name:?} must be rejected");
        }
    }
}
