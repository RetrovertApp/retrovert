//! On-disk layout of a channel: a self-contained TUF repository.

use std::path::{Path, PathBuf};

use crate::error::{Error, Result};
use crate::metadata::RoleName;

/// The file names a role's metadata is published under, given consistent
/// snapshots.
///
/// Root is published twice: clients bootstrap from `root.json` and walk the
/// rotation chain by version. Timestamp is never version-prefixed — it is the
/// fixed entry point a client polls, so its name cannot depend on a version the
/// client does not yet know.
#[must_use]
pub fn published_names(role: RoleName, version: u64) -> Vec<String> {
    let unversioned = role.file_name();
    match role {
        RoleName::Timestamp => vec![unversioned],
        RoleName::Root => vec![format!("{version}.{unversioned}"), unversioned],
        RoleName::Targets | RoleName::Snapshot => vec![format!("{version}.{unversioned}")],
    }
}

/// A channel directory: `metadata/` beside `targets/`.
#[derive(Debug, Clone)]
pub struct Channel {
    path: PathBuf,
}

impl Channel {
    /// Address the channel rooted at `path`. Nothing is touched until
    /// [`Channel::create_dirs`] or [`Channel::write_metadata`] is called.
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// The channel root.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Where signed role metadata lives.
    #[must_use]
    pub fn metadata_dir(&self) -> PathBuf {
        self.path.join("metadata")
    }

    /// Where target files live.
    #[must_use]
    pub fn targets_dir(&self) -> PathBuf {
        self.path.join("targets")
    }

    /// Create the metadata and targets directories.
    pub fn create_dirs(&self) -> Result<()> {
        for dir in [self.metadata_dir(), self.targets_dir()] {
            std::fs::create_dir_all(&dir).map_err(|e| Error::io(dir, e))?;
        }
        Ok(())
    }

    /// Write one metadata file.
    pub fn write_metadata(&self, file_name: &str, bytes: &[u8]) -> Result<()> {
        let path = self.metadata_dir().join(file_name);
        std::fs::write(&path, bytes).map_err(|e| Error::io(path, e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn consistent_snapshot_naming() {
        assert_eq!(
            published_names(RoleName::Root, 1),
            ["1.root.json", "root.json"]
        );
        assert_eq!(published_names(RoleName::Snapshot, 3), ["3.snapshot.json"]);
        assert_eq!(published_names(RoleName::Targets, 3), ["3.targets.json"]);
        assert_eq!(published_names(RoleName::Timestamp, 3), ["timestamp.json"]);
    }
}
