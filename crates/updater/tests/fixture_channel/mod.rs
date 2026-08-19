//! A channel published into a temporary directory.
//!
//! Signed with the same shared model crate the publisher signs with, in the
//! same order, under the same consistent-snapshot names — so what a client
//! reads here is shaped like what a live channel serves. It is built in-process
//! rather than by shelling out to the publish CLI because that CLI lives in the
//! repository which depends on this one, and because a fixture channel has to
//! be reproducible offline and datable to any instant a test needs.

// Each test binary compiles this module separately and uses a different subset.
#![allow(dead_code)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use jiff::Timestamp;
use retrovert_tuf::manifest;
use retrovert_tuf::metadata::sha256_map;
use retrovert_tuf::{
    Channel as TufChannel, KeyPair, MetaFile, PublicKey, RoleName, Root, Signed, Snapshot,
    TargetFile, Targets, policy, published_names, target_published_name,
};
use sigstore_tuf::Error as TufError;
use sigstore_tuf::transport::{FetchFuture, Repository};
use tempfile::TempDir;

/// One signing key per top-level role.
#[derive(Debug, Clone)]
pub struct KeySet {
    pub root: KeyPair,
    pub targets: KeyPair,
    pub snapshot: KeyPair,
    pub timestamp: KeyPair,
}

impl KeySet {
    /// Four keys derived from `seed`, so a fixture is reproducible. Seeds four
    /// or more apart give two fixtures no key in common.
    pub fn seeded(seed: u8) -> Self {
        Self {
            root: KeyPair::from_seed(&[seed; 32]),
            targets: KeyPair::from_seed(&[seed.wrapping_add(1); 32]),
            snapshot: KeyPair::from_seed(&[seed.wrapping_add(2); 32]),
            timestamp: KeyPair::from_seed(&[seed.wrapping_add(3); 32]),
        }
    }

    fn public_keys(&self) -> Vec<(RoleName, PublicKey)> {
        vec![
            (RoleName::Root, self.root.public()),
            (RoleName::Targets, self.targets.public()),
            (RoleName::Snapshot, self.snapshot.public()),
            (RoleName::Timestamp, self.timestamp.public()),
        ]
    }
}

/// A published channel and the state its next publication advances from.
pub struct FixtureChannel {
    dir: TempDir,
    channel: TufChannel,
    keys: KeySet,
    targets: BTreeMap<String, TargetFile>,
    targets_version: u64,
    snapshot_version: u64,
    timestamp_version: u64,
}

impl FixtureChannel {
    /// An empty channel signed by seed-1 keys, dated `at`.
    pub fn init(at: Timestamp) -> Self {
        Self::init_with(KeySet::seeded(1), at)
    }

    /// An empty channel signed by `keys`, dated `at`.
    pub fn init_with(keys: KeySet, at: Timestamp) -> Self {
        let dir = TempDir::new().expect("a temporary directory");
        let channel = TufChannel::new(dir.path().join("channel"));
        channel.create_dirs().expect("channel directories");

        let root = Signed::new(
            Root::single_key_per_role(
                1,
                policy::expires(RoleName::Root, at).expect("a root expiry"),
                &keys.public_keys(),
            )
            .expect("a root role"),
            &[&keys.root],
        )
        .expect("a signed root")
        .to_json()
        .expect("root json");
        for name in published_names(RoleName::Root, 1) {
            channel.write_metadata(&name, &root).expect("root written");
        }

        let mut fixture = Self {
            dir,
            channel,
            keys,
            targets: BTreeMap::new(),
            targets_version: 0,
            snapshot_version: 0,
            timestamp_version: 0,
        };
        fixture.commit(at);
        fixture
    }

    /// Publish `manifest_bytes` as the channel's sole target, dated `at`, and
    /// return the generation id.
    pub fn publish(&mut self, manifest_bytes: &[u8], at: Timestamp) -> String {
        let generation_id = manifest::generation_id(manifest_bytes);
        let name = target_published_name(manifest::TARGET_PATH, &generation_id);
        self.channel
            .write_target(&name, manifest_bytes)
            .expect("target written");

        self.targets = BTreeMap::from([(
            manifest::TARGET_PATH.to_string(),
            TargetFile {
                length: manifest_bytes.len() as u64,
                hashes: sha256_map(manifest_bytes),
                extra: BTreeMap::new(),
            },
        )]);
        self.commit(at);
        generation_id
    }

    /// Re-sign the online roles at `at`, leaving the target set alone — what
    /// the scheduled job does to push expiries out.
    pub fn resign(&mut self, at: Timestamp) {
        self.commit(at);
    }

    /// The root a client embeds to authenticate this channel.
    pub fn root(&self) -> Vec<u8> {
        std::fs::read(self.channel.metadata_dir().join(RoleName::Root.file_name()))
            .expect("a published root")
    }

    /// The channel directory.
    pub fn path(&self) -> &Path {
        self.channel.path()
    }

    /// The channel as a client reads it: metadata and targets in one flat
    /// namespace.
    pub fn source(&self) -> Arc<dyn Repository> {
        Arc::new(FlatDirs::new(self))
    }

    /// The channel with `name` answered from `bytes` instead of from disk —
    /// how a test puts bytes on the wire that no key ever signed.
    pub fn tampered(&self, name: &str, bytes: Vec<u8>) -> Arc<dyn Repository> {
        let mut source = FlatDirs::new(self);
        source.overrides.insert(name.to_string(), bytes);
        Arc::new(source)
    }

    /// An independent copy of the channel exactly as it stands, which keeps
    /// serving this generation while the original moves on.
    pub fn frozen_copy(&self) -> FrozenChannel {
        let dir = TempDir::new().expect("a temporary directory");
        let root = dir.path().join("channel");
        copy_dir(self.channel.path(), &root);
        FrozenChannel { dir, root }
    }

    /// Every published file, keyed by the flat asset name it is served under.
    pub fn assets(&self) -> BTreeMap<String, Vec<u8>> {
        assets_of(&[self.channel.metadata_dir(), self.channel.targets_dir()])
    }

    fn commit(&mut self, at: Timestamp) {
        self.targets_version += 1;
        self.snapshot_version += 1;
        self.timestamp_version += 1;

        let targets = Signed::new(
            Targets::new(
                self.targets_version,
                policy::expires(RoleName::Targets, at).expect("a targets expiry"),
                self.targets.clone(),
            ),
            &[&self.keys.targets],
        )
        .expect("a signed targets")
        .to_json()
        .expect("targets json");

        let snapshot = Signed::new(
            Snapshot::new(
                self.snapshot_version,
                policy::expires(RoleName::Snapshot, at).expect("a snapshot expiry"),
                BTreeMap::from([(
                    RoleName::Targets.file_name(),
                    MetaFile::pinning(self.targets_version, &targets),
                )]),
            ),
            &[&self.keys.snapshot],
        )
        .expect("a signed snapshot")
        .to_json()
        .expect("snapshot json");

        let timestamp = Signed::new(
            retrovert_tuf::Timestamp::new(
                self.timestamp_version,
                policy::expires(RoleName::Timestamp, at).expect("a timestamp expiry"),
                MetaFile::pinning(self.snapshot_version, &snapshot),
            ),
            &[&self.keys.timestamp],
        )
        .expect("a signed timestamp")
        .to_json()
        .expect("timestamp json");

        for (role, version, bytes) in [
            (RoleName::Targets, self.targets_version, targets),
            (RoleName::Snapshot, self.snapshot_version, snapshot),
            (RoleName::Timestamp, self.timestamp_version, timestamp),
        ] {
            for name in published_names(role, version) {
                self.channel
                    .write_metadata(&name, &bytes)
                    .expect("metadata written");
            }
        }
    }
}

/// A copy of a channel that no longer advances.
pub struct FrozenChannel {
    dir: TempDir,
    root: PathBuf,
}

impl FrozenChannel {
    /// The frozen channel as a client reads it.
    pub fn source(&self) -> Arc<dyn Repository> {
        Arc::new(FlatDirs {
            dirs: vec![self.root.join("metadata"), self.root.join("targets")],
            overrides: BTreeMap::new(),
        })
    }
}

/// Serves named files out of several directories, first match wins — the flat
/// asset namespace a release exposes, where a file's directory is not part of
/// its name.
struct FlatDirs {
    dirs: Vec<PathBuf>,
    overrides: BTreeMap<String, Vec<u8>>,
}

impl FlatDirs {
    fn new(fixture: &FixtureChannel) -> Self {
        Self {
            dirs: vec![
                fixture.channel.metadata_dir(),
                fixture.channel.targets_dir(),
            ],
            overrides: BTreeMap::new(),
        }
    }

    fn read(&self, name: &str, max_length: u64) -> sigstore_tuf::Result<Option<Vec<u8>>> {
        let found = self.overrides.get(name).cloned().or_else(|| {
            self.dirs
                .iter()
                .find_map(|dir| std::fs::read(dir.join(name)).ok())
        });
        match found {
            Some(bytes) if bytes.len() as u64 > max_length => Err(TufError::Transport(format!(
                "{name} exceeds max length {max_length}"
            ))),
            other => Ok(other),
        }
    }
}

impl Repository for FlatDirs {
    fn fetch_metadata<'a>(&'a self, name: &'a str, max_length: u64) -> FetchFuture<'a> {
        let result = self.read(name, max_length);
        Box::pin(async move { result })
    }

    fn fetch_target<'a>(&'a self, path: &'a str, max_length: u64) -> FetchFuture<'a> {
        let result = self.read(path, max_length);
        Box::pin(async move { result })
    }
}

/// A release-set manifest naming one artifact, at `version`.
pub fn manifest_bytes(version: u64, revision: &str) -> Vec<u8> {
    serde_json::to_vec_pretty(&serde_json::json!({
        "schema": manifest::SCHEMA_VERSION,
        "version": version,
        "source_revision": revision,
        "published": "2026-08-15T12:00:00Z",
        "artifacts": [{
            "name": "spu",
            "target": "linux-x86_64",
            "path": format!("spu-{revision}-linux-x86_64.tar.zst"),
            "sha256": "ab".repeat(32),
            "size": 42,
            "revision": revision,
        }],
    }))
    .expect("manifest json")
}

fn assets_of(dirs: &[PathBuf]) -> BTreeMap<String, Vec<u8>> {
    let mut assets = BTreeMap::new();
    for dir in dirs {
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        for entry in entries.flatten() {
            if let Ok(bytes) = std::fs::read(entry.path()) {
                assets.insert(entry.file_name().to_string_lossy().into_owned(), bytes);
            }
        }
    }
    assets
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).expect("a destination directory");
    for entry in std::fs::read_dir(from)
        .expect("a readable directory")
        .flatten()
    {
        let target = to.join(entry.file_name());
        if entry.file_type().expect("a file type").is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), &target).expect("a copied file");
        }
    }
}
