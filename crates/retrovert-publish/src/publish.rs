//! `publish`: commit a release-set manifest to a channel as its next
//! generation.
//!
//! The manifest becomes the channel's sole TUF target. Files land in
//! publication order — target, targets, snapshot, then timestamp — and every
//! file before the last gets a fresh consistent-snapshot name, so an
//! interrupted publish leaves the previous generation untouched. The atomic
//! `timestamp.json` write is the commit point.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use jiff::Timestamp;
use retrovert_tuf::{
    MetaFile, RoleName, Signed, Snapshot, TargetFile, Targets, manifest, metadata::sha256_map,
    policy, published_names, target_published_name,
};
use serde::de::DeserializeOwned;

use crate::error::{Error, Result};
use crate::workspace::Workspace;

/// What `publish` wrote.
#[derive(Debug, Clone)]
pub struct PublishReport {
    /// The generation id: the digest of the manifest's exact bytes.
    pub generation_id: String,
    /// The manifest's source revision.
    pub revision: String,
    /// The manifest's display version, if it declares one.
    pub version: Option<String>,
    /// Every file written, in publication order; the closing `timestamp.json`
    /// write commits the generation.
    pub written: Vec<PathBuf>,
}

/// Publish the release-set manifest at `manifest_path` into `workspace`'s
/// channel, dated `now`.
///
/// Re-publishing byte-identical manifest bytes reproduces the same generation
/// id; metadata versions still advance, so clients see a fresh chain either
/// way.
pub fn publish(
    workspace: &Workspace,
    manifest_path: &Path,
    now: Timestamp,
) -> Result<PublishReport> {
    let manifest_bytes = std::fs::read(manifest_path).map_err(|e| Error::io(manifest_path, e))?;
    let manifest = manifest::Manifest::parse(&manifest_bytes)?;
    let generation_id = manifest::generation_id(&manifest_bytes);

    let store = workspace.keys();
    let channel = workspace.channel();
    let metadata_dir = channel.metadata_dir();

    let timestamp_path = metadata_dir.join(RoleName::Timestamp.file_name());
    let current_timestamp: Signed<retrovert_tuf::Timestamp> = read_signed(&timestamp_path)?;
    let snapshot_pin = pinned_version(
        &current_timestamp.signed.meta,
        &timestamp_path,
        RoleName::Snapshot,
    )?;

    let snapshot_path = metadata_dir.join(&published_names(RoleName::Snapshot, snapshot_pin)[0]);
    let current_snapshot: Signed<Snapshot> = read_signed(&snapshot_path)?;

    let targets_version = next_version(
        pinned_version(
            &current_snapshot.signed.meta,
            &snapshot_path,
            RoleName::Targets,
        )?,
        &snapshot_path,
    )?;
    let snapshot_version = next_version(current_snapshot.signed.version, &snapshot_path)?;
    let timestamp_version = next_version(current_timestamp.signed.version, &timestamp_path)?;

    let manifest_entry = TargetFile {
        length: manifest_bytes.len() as u64,
        hashes: sha256_map(&manifest_bytes),
        custom: None,
        extra: BTreeMap::new(),
    };
    let targets = Signed::new(
        Targets::new(
            targets_version,
            policy::expires(RoleName::Targets, now)?,
            BTreeMap::from([(manifest::TARGET_PATH.to_string(), manifest_entry)]),
        ),
        &[&store.read(RoleName::Targets)?],
    )?
    .to_json()?;

    let snapshot = Signed::new(
        Snapshot::new(
            snapshot_version,
            policy::expires(RoleName::Snapshot, now)?,
            BTreeMap::from([(
                RoleName::Targets.file_name(),
                MetaFile::pinning(targets_version, &targets),
            )]),
        ),
        &[&store.read(RoleName::Snapshot)?],
    )?
    .to_json()?;

    let timestamp = Signed::new(
        retrovert_tuf::Timestamp::new(
            timestamp_version,
            policy::expires(RoleName::Timestamp, now)?,
            MetaFile::pinning(snapshot_version, &snapshot),
        ),
        &[&store.read(RoleName::Timestamp)?],
    )?
    .to_json()?;

    let target_name = target_published_name(manifest::TARGET_PATH, &generation_id);
    channel.write_target(&target_name, &manifest_bytes)?;
    let mut written = vec![channel.targets_dir().join(&target_name)];

    for (role, version, bytes) in [
        (RoleName::Targets, targets_version, &targets),
        (RoleName::Snapshot, snapshot_version, &snapshot),
        (RoleName::Timestamp, timestamp_version, &timestamp),
    ] {
        for name in published_names(role, version) {
            channel.write_metadata(&name, bytes)?;
            written.push(metadata_dir.join(name));
        }
    }

    Ok(PublishReport {
        generation_id,
        revision: manifest.revision,
        version: manifest.version,
        written,
    })
}

fn read_signed<T: DeserializeOwned>(path: &Path) -> Result<Signed<T>> {
    let bytes = std::fs::read(path).map_err(|e| Error::io(path, e))?;
    serde_json::from_slice(&bytes).map_err(|e| Error::Metadata {
        path: path.to_path_buf(),
        source: e,
    })
}

/// Advance a version counter read from `path`, refusing to wrap.
fn next_version(version: u64, path: &Path) -> Result<u64> {
    version
        .checked_add(1)
        .ok_or_else(|| Error::VersionExhausted {
            path: path.to_path_buf(),
            version,
        })
}

fn pinned_version(meta: &BTreeMap<String, MetaFile>, path: &Path, child: RoleName) -> Result<u64> {
    let name = child.file_name();
    match meta.get(&name) {
        Some(pin) => Ok(pin.version),
        None => Err(Error::MissingPin {
            path: path.to_path_buf(),
            name,
        }),
    }
}
