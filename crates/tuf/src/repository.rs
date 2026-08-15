//! On-disk layout of a channel: a self-contained TUF repository.

use std::io::Write;
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

    /// Write one metadata file, atomically.
    ///
    /// Publication order gives a channel its consistency — a role is written
    /// only after everything it pins — and that argument holds only if each
    /// individual write is indivisible too. Overwriting `timestamp.json` in
    /// place would otherwise leave a window where a client polling the channel
    /// reads a truncated file.
    pub fn write_metadata(&self, file_name: &str, bytes: &[u8]) -> Result<()> {
        write_atomically(&self.metadata_dir().join(file_name), bytes)
    }
}

/// Write `bytes` to `path` so that a concurrent reader sees either the previous
/// contents or the complete new ones, never a partial write.
///
/// The temporary file is created in the destination directory because `rename`
/// is only atomic within a filesystem, and is flushed before the rename so a
/// crash cannot leave the destination name pointing at unwritten data.
fn write_atomically(path: &Path, bytes: &[u8]) -> Result<()> {
    let (Some(dir), Some(name)) = (path.parent(), path.file_name().and_then(|n| n.to_str())) else {
        return Err(Error::io(
            path,
            std::io::Error::other("not a writable file path"),
        ));
    };
    // Process ID keeps two publishers writing the same channel from colliding
    // on the temporary name; the leading dot keeps it out of directory listings
    // if one is ever left behind by a crash.
    let temp = dir.join(format!(".{name}.{}.tmp", std::process::id()));

    let write = || -> std::io::Result<()> {
        let mut file = std::fs::File::create(&temp)?;
        file.write_all(bytes)?;
        file.sync_all()
    };
    if let Err(e) = write() {
        // INTENTIONAL: cleanup is best-effort; the write error is what matters.
        drop(std::fs::remove_file(&temp));
        return Err(Error::io(&temp, e));
    }

    std::fs::rename(&temp, path).map_err(|e| {
        // INTENTIONAL: as above — do not mask the rename failure.
        drop(std::fs::remove_file(&temp));
        Error::io(path, e)
    })
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

    fn channel() -> (tempfile::TempDir, Channel) {
        let dir = tempfile::TempDir::new().unwrap();
        let channel = Channel::new(dir.path().join("repository"));
        channel.create_dirs().unwrap();
        (dir, channel)
    }

    #[test]
    fn metadata_writes_replace_in_place_and_leave_no_temporaries() {
        let (_dir, channel) = channel();
        channel.write_metadata("timestamp.json", b"first").unwrap();
        channel.write_metadata("timestamp.json", b"second").unwrap();

        let entries: Vec<String> = std::fs::read_dir(channel.metadata_dir())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(entries, ["timestamp.json"]);
        assert_eq!(
            std::fs::read(channel.metadata_dir().join("timestamp.json")).unwrap(),
            b"second"
        );
    }

    #[test]
    fn a_write_to_a_missing_directory_fails_without_leaving_a_temporary() {
        let dir = tempfile::TempDir::new().unwrap();
        let channel = Channel::new(dir.path().join("repository"));

        channel
            .write_metadata("timestamp.json", b"payload")
            .expect_err("metadata directory was never created");
        assert!(!channel.metadata_dir().exists());
    }
}
