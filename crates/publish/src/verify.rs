//! Reading a channel back the way a client does: a base URL or a directory, and
//! a pinned root.
//!
//! Nothing else is needed and nothing else is trusted. The source serves bytes;
//! every claim about which generation is current comes from signatures chaining
//! to the root the caller supplied. Both readers here go through the same TUF
//! client a device runs, because a publisher checking its own work against its
//! own idea of the rules checks nothing.

use std::path::{Path, PathBuf};

use jiff::Timestamp;
use retrovert_tuf::{Channel, RoleName, Root, Signed, manifest};
use sigstore_tuf::transport::{FetchFuture, Repository};
use sigstore_tuf::{Error as ClientError, Updater};

use crate::chain::SignedRole;
use crate::error::{Error, Result};
use crate::workspace::Workspace;

/// A channel served over HTTPS from one flat namespace.
///
/// Release assets have no directories, so a channel's metadata and its target
/// sit side by side under a single base URL — which is exactly the shape TUF's
/// consistent-snapshot naming already guarantees is collision-free.
pub struct HttpChannel {
    base: String,
    agent: ureq::Agent,
}

impl HttpChannel {
    /// Serve the channel published at `base_url`.
    #[must_use]
    pub fn new(base_url: &str) -> Self {
        Self {
            base: format!("{}/", base_url.trim_end_matches('/')),
            agent: crate::host::agent(),
        }
    }

    fn fetch(&self, name: &str, max_length: u64) -> sigstore_tuf::Result<Option<Vec<u8>>> {
        let name = flat_name(name)?;
        let url = format!("{}{name}{}", self.base, cache_key(name));

        let mut response = self
            .agent
            .get(&url)
            .header("cache-control", "no-cache")
            .header("pragma", "no-cache")
            .call()
            .map_err(|e| ClientError::Transport(format!("GET {url}: {e}")))?;
        match response.status().as_u16() {
            200 => {}
            404 => return Ok(None),
            status => {
                return Err(ClientError::Transport(format!("GET {url}: HTTP {status}")));
            }
        }

        // Bound the read itself rather than the result: an endless-data mirror
        // must not be able to spend our memory before we notice. One byte of
        // headroom is read past the bound so that a body of exactly
        // `max_length` — which is every length-pinned target — is not mistaken
        // for an overrun, and rejected here instead.
        let bytes = response
            .body_mut()
            .with_config()
            .limit(max_length.saturating_add(1))
            .read_to_vec()
            .map_err(|e| ClientError::Transport(format!("GET {url}: {e}")))?;
        if bytes.len() as u64 > max_length {
            return Err(ClientError::Transport(format!(
                "GET {url}: body exceeds {max_length} bytes"
            )));
        }
        Ok(Some(bytes))
    }
}

impl Repository for HttpChannel {
    fn fetch_metadata<'a>(&'a self, name: &'a str, max_length: u64) -> FetchFuture<'a> {
        let result = self.fetch(name, max_length);
        Box::pin(async move { result })
    }

    fn fetch_target<'a>(&'a self, path: &'a str, max_length: u64) -> FetchFuture<'a> {
        let result = self.fetch(path, max_length);
        Box::pin(async move { result })
    }
}

/// The generation a channel currently names.
#[derive(Debug, Clone)]
pub struct Generation {
    /// The digest of the manifest's exact bytes.
    pub generation_id: String,
    /// The channel's release-set number.
    pub version: u64,
    /// The aggregate repository commit the release set was gathered from.
    pub source_revision: String,
    /// When the set was published, RFC 3339 in UTC.
    pub published: String,
    /// How many artifacts the set ships.
    pub artifacts: usize,
}

/// A channel read off a directory rather than a network.
///
/// A workspace's `repository/` is byte-for-byte what a client sees at the base
/// URL, so the same TUF client can be pointed at it — which is the only way to
/// check a channel on a machine that has no network, and the only check worth
/// having at a root ceremony.
pub struct LocalChannel {
    metadata: PathBuf,
    targets: PathBuf,
}

impl LocalChannel {
    /// Read `channel`'s published directories.
    #[must_use]
    pub fn new(channel: &Channel) -> Self {
        Self {
            metadata: channel.metadata_dir(),
            targets: channel.targets_dir(),
        }
    }

    fn read(path: &Path, max_length: u64) -> sigstore_tuf::Result<Option<Vec<u8>>> {
        match std::fs::read(path) {
            Ok(bytes) if bytes.len() as u64 > max_length => Err(ClientError::Transport(format!(
                "{} exceeds max length {max_length}",
                path.display()
            ))),
            Ok(bytes) => Ok(Some(bytes)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(ClientError::Transport(e.to_string())),
        }
    }
}

impl Repository for LocalChannel {
    fn fetch_metadata<'a>(&'a self, name: &'a str, max_length: u64) -> FetchFuture<'a> {
        // Names are flattened here for the same reason they are over HTTPS: a
        // name that reached out of the directory would resolve something the
        // channel never published.
        let result =
            flat_name(name).and_then(|name| Self::read(&self.metadata.join(name), max_length));
        Box::pin(async move { result })
    }

    fn fetch_target<'a>(&'a self, path: &'a str, max_length: u64) -> FetchFuture<'a> {
        let result =
            flat_name(path).and_then(|name| Self::read(&self.targets.join(name), max_length));
        Box::pin(async move { result })
    }
}

/// A channel's metadata chain as a client trusts it.
#[derive(Debug, Clone)]
pub struct Chain {
    /// The `root` role's key IDs, sorted.
    pub root_key_ids: Vec<String>,
    /// How many of them a root signature set needs.
    pub root_threshold: u32,
    /// How many signatures the served root actually carries.
    pub root_signatures: usize,
    /// Every role's version and expiry, in publication order.
    pub roles: Vec<SignedRole>,
}

/// Refresh `workspace`'s own channel against its own root, offline, as of
/// `now`.
///
/// This is what a ceremony can check before anyone leaves the room: that the
/// root just written meets its own threshold, and that the chain under it
/// verifies through the same client library a device runs. It resolves no
/// generation, because a channel that has just been created names none.
pub fn check(workspace: &Workspace, now: Timestamp) -> Result<Chain> {
    let channel = workspace.channel();
    let root_path = channel.metadata_dir().join(RoleName::Root.file_name());
    let root_bytes = std::fs::read(&root_path).map_err(|e| Error::io(&root_path, e))?;
    let root: Signed<Root> =
        serde_json::from_slice(&root_bytes).map_err(|source| Error::Metadata {
            path: root_path,
            source,
        })?;
    let role = root
        .signed
        .roles
        .get(RoleName::Root.as_str())
        .ok_or_else(|| Error::MissingPin {
            path: channel.metadata_dir().join(RoleName::Root.file_name()),
            name: RoleName::Root.file_name(),
        })?;

    let mut updater = Updater::new(LocalChannel::new(&channel), &root_bytes)?;
    pollster::block_on(updater.refresh(now))?;
    let trusted = updater.trusted();

    // In publication order, and only what the refresh actually established: a
    // role the client did not load is a role this has nothing to say about, and
    // reporting it as version zero would say something false.
    let mut roles = vec![SignedRole {
        role: RoleName::Root,
        version: trusted.root().version,
        expires: trusted.root().expires.clone(),
    }];
    if let Some(targets) = trusted.targets() {
        roles.push(SignedRole {
            role: RoleName::Targets,
            version: targets.version,
            expires: targets.expires.clone(),
        });
    }
    if let Some(snapshot) = trusted.snapshot() {
        roles.push(SignedRole {
            role: RoleName::Snapshot,
            version: snapshot.version,
            expires: snapshot.expires.clone(),
        });
    }
    if let Some(timestamp) = trusted.timestamp() {
        roles.push(SignedRole {
            role: RoleName::Timestamp,
            version: timestamp.version,
            expires: timestamp.expires.clone(),
        });
    }

    Ok(Chain {
        root_key_ids: role.keyids.clone(),
        root_threshold: role.threshold,
        root_signatures: root.signatures.len(),
        roles,
    })
}

/// Refresh the channel at `base_url` against `root`, then resolve the manifest
/// it names, as of `now`.
pub fn verify(base_url: &str, root: &[u8], now: Timestamp) -> Result<Generation> {
    let mut updater = Updater::new(HttpChannel::new(base_url), root)?;
    pollster::block_on(updater.refresh(now))?;
    let bytes = pollster::block_on(updater.get_target(manifest::TARGET_PATH, now))?;

    let manifest = manifest::Manifest::parse(&bytes)?;
    Ok(Generation {
        generation_id: manifest::generation_id(&bytes),
        version: manifest.version,
        source_revision: manifest.source_revision,
        published: manifest.published,
        artifacts: manifest.artifacts.len(),
    })
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
fn cache_key(name: &str) -> String {
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
        return Err(ClientError::Transport(format!(
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
                HttpChannel::new(given).base,
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
