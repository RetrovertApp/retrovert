//! Helpers shared by the publisher's integration tests.

// Each test binary compiles this module separately and uses a different subset.
#![allow(dead_code)]

use std::path::{Path, PathBuf};

use jiff::Timestamp;
use retrovert_publish::{KeySet, Workspace, init};
use retrovert_tuf::KeyPair;
use sigstore_tuf::transport::FetchFuture;
use sigstore_tuf::{Repository, Updater};
use tempfile::TempDir;

/// A fixed instant, so every expiry assertion has an exact expected value.
pub fn now() -> Timestamp {
    "2026-08-15T12:00:00Z".parse().unwrap()
}

/// Fixed keys, so publisher output is reproducible byte for byte.
pub fn seeded_keys() -> KeySet {
    KeySet {
        root: KeyPair::from_seed(&[1u8; 32]),
        targets: KeyPair::from_seed(&[2u8; 32]),
        snapshot: KeyPair::from_seed(&[3u8; 32]),
        timestamp: KeyPair::from_seed(&[4u8; 32]),
    }
}

pub fn seeded_workspace() -> (TempDir, Workspace) {
    let dir = TempDir::new().unwrap();
    let workspace = Workspace::new(dir.path().join("channel"));
    init(&workspace, &seeded_keys(), now(), false).unwrap();
    (dir, workspace)
}

pub fn metadata_dir(workspace: &Workspace) -> PathBuf {
    workspace.channel().metadata_dir()
}

pub fn read(workspace: &Workspace, name: &str) -> Vec<u8> {
    std::fs::read(metadata_dir(workspace).join(name)).unwrap()
}

/// Copy a workspace tree, so tests can replay publish steps on a clone.
pub fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), &target).unwrap();
        }
    }
}

/// Serves a channel's `metadata/` and `targets/` directories to `sigstore-tuf`
/// the way an HTTP mirror would expose its two base URLs, offline.
pub struct ChannelRepository {
    metadata: PathBuf,
    targets: PathBuf,
}

impl ChannelRepository {
    pub fn new(workspace: &Workspace) -> Self {
        let channel = workspace.channel();
        Self {
            metadata: channel.metadata_dir(),
            targets: channel.targets_dir(),
        }
    }
}

fn read_limited(path: &Path, max_length: u64) -> sigstore_tuf::Result<Option<Vec<u8>>> {
    match std::fs::read(path) {
        Ok(bytes) if bytes.len() as u64 > max_length => Err(sigstore_tuf::Error::Transport(
            format!("{} exceeds max length {max_length}", path.display()),
        )),
        Ok(bytes) => Ok(Some(bytes)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(sigstore_tuf::Error::Transport(e.to_string())),
    }
}

impl Repository for ChannelRepository {
    fn fetch_metadata<'a>(&'a self, name: &'a str, max_length: u64) -> FetchFuture<'a> {
        let result = read_limited(&self.metadata.join(name), max_length);
        Box::pin(async move { result })
    }

    fn fetch_target<'a>(&'a self, path: &'a str, max_length: u64) -> FetchFuture<'a> {
        let result = read_limited(&self.targets.join(path), max_length);
        Box::pin(async move { result })
    }
}

/// Bootstrap `sigstore-tuf` from the channel's root and run its refresh
/// workflow, offline.
///
/// [`ChannelRepository`] is synchronous under an async trait, so the futures
/// are always ready and a trivial `block_on` is enough — no runtime needed.
pub fn refresh_with_sigstore_tuf(
    workspace: &Workspace,
    at: Timestamp,
) -> sigstore_tuf::Result<Updater> {
    let root = read(workspace, "root.json");
    let mut updater = Updater::new(ChannelRepository::new(workspace), &root)?;
    pollster::block_on(updater.refresh(at))?;
    Ok(updater)
}
