//! Authenticating an update channel: a base URL and an embedded root in, a
//! release manifest or a refusal out.
//!
//! Nothing the host says is taken on its word. Bytes are accepted only once
//! they chain to the root compiled into this binary, only against a time the
//! network supplied, and only if the chain is no older than one this client has
//! already trusted.

mod error;
mod source;
mod time;
mod trust;

#[cfg(test)]
mod authentication;

use std::sync::Arc;

use jiff::Timestamp;
use retrovert_tuf::manifest::{self, Manifest};
use sigstore_tuf::transport::FetchFuture;
use sigstore_tuf::{Repository, Updater};

pub use error::{Error, Result};
pub use source::HttpSource;
#[cfg_attr(not(test), allow(unused_imports))]
pub use time::NetworkTime;
pub use time::{Clock, HostDate};
pub use trust::{Floor, TrustStore};

use crate::transport::Transport;

/// What one authentication attempt produced.
#[derive(Debug, Clone)]
pub enum Attempt {
    /// No network time was available, so nothing was verified and no trust
    /// state moved. The caller keeps whatever generation it already has.
    Skipped,
    /// The channel authenticated to a manifest.
    Authenticated(Authenticated),
}

/// A manifest the channel's signatures vouch for.
#[derive(Debug, Clone)]
pub struct Authenticated {
    /// The release set the channel currently names.
    pub manifest: Manifest,
    /// The digest of the manifest's exact bytes.
    pub generation_id: String,
    /// The network time the chain was verified against.
    #[cfg_attr(not(test), allow(dead_code))]
    pub verified_at: Timestamp,
}

/// A signed update channel, read through a source and authenticated against an
/// embedded root.
pub struct Channel {
    source: Arc<dyn Repository>,
    clock: Arc<dyn Clock>,
    root: Vec<u8>,
    trust: TrustStore,
}

impl std::fmt::Debug for Channel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Channel")
            .field("trust", &self.trust.dir())
            .finish_non_exhaustive()
    }
}

impl Channel {
    /// The channel served at `base_url`, authenticated against `root` and
    /// recording its trust state in `trust`.
    ///
    /// HTTPS only, because the verification time is the host's `Date` header.
    pub fn https(
        transport: &Arc<Transport>,
        base_url: &str,
        root: Vec<u8>,
        trust: TrustStore,
    ) -> Result<Self> {
        let clock = HostDate::new(Arc::clone(transport), base_url)?;
        let source = HttpSource::new(Arc::clone(transport), base_url);
        Ok(Self::new(Arc::new(source), Arc::new(clock), root, trust))
    }

    /// The channel read through `source` and timed by `clock`.
    #[must_use]
    pub fn new(
        source: Arc<dyn Repository>,
        clock: Arc<dyn Clock>,
        root: Vec<u8>,
        trust: TrustStore,
    ) -> Self {
        Self {
            source,
            clock,
            root,
            trust,
        }
    }

    /// Resolve the channel to the manifest it currently names.
    ///
    /// The refresh starts from the embedded root every time and walks forward,
    /// so a trust directory can raise what is accepted but never lower it: the
    /// worst a wiped one does is return this client to the embedded root, which
    /// is where a factory reset is meant to leave it.
    ///
    /// Exclusive, because raising the floor is a read-modify-write over the
    /// trust directory: two checks running against one directory could
    /// otherwise interleave and leave the lower of their two floors behind.
    pub fn authenticate(&mut self) -> Result<Attempt> {
        let Some(now) = self.clock.network_time() else {
            return Ok(Attempt::Skipped);
        };
        let at = now.timestamp();

        let mut updater = Updater::new(Shared(Arc::clone(&self.source)), &self.root)?
            .with_store(self.trust.metadata());
        pollster::block_on(updater.refresh(at))?;

        // Recorded as soon as the chain verifies, before the target is
        // resolved: the floor is a statement about the metadata, and a manifest
        // that fails to parse does not make the chain that carried it older.
        let offered = Floor::of(updater.trusted())?;
        let floor = self.trust.floor()?;
        floor.admit(&offered)?;
        self.trust.record(&floor.raised_to(&offered))?;

        let bytes = pollster::block_on(updater.get_target(manifest::TARGET_PATH, at))?;
        Ok(Attempt::Authenticated(Authenticated {
            generation_id: manifest::generation_id(&bytes),
            manifest: Manifest::parse(&bytes)?,
            verified_at: at,
        }))
    }
}

/// Lends the channel's source to the fresh [`Updater`] each check builds.
///
/// A check starts from the embedded root and re-seeds its floor from disk, so
/// it needs an updater of its own; the source outlives all of them.
struct Shared(Arc<dyn Repository>);

impl Repository for Shared {
    fn fetch_metadata<'a>(&'a self, name: &'a str, max_length: u64) -> FetchFuture<'a> {
        self.0.fetch_metadata(name, max_length)
    }

    fn fetch_target<'a>(&'a self, path: &'a str, max_length: u64) -> FetchFuture<'a> {
        self.0.fetch_target(path, max_length)
    }
}
