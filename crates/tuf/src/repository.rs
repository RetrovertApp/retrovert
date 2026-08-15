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

/// The file name a target file is published under with consistent snapshots:
/// the digest prefixes the file name only, so `dir/name` becomes
/// `dir/<sha256>.name`.
#[must_use]
pub fn target_published_name(target_path: &str, sha256_hex: &str) -> String {
    match target_path.rsplit_once('/') {
        Some((dir, name)) => format!("{dir}/{sha256_hex}.{name}"),
        None => format!("{sha256_hex}.{target_path}"),
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

    /// Write one target file, atomically, for the same reason as
    /// [`Channel::write_metadata`].
    pub fn write_target(&self, file_name: &str, bytes: &[u8]) -> Result<()> {
        write_atomically(&self.targets_dir().join(file_name), bytes)
    }
}

/// Write `bytes` to `path` so that a concurrent reader sees either the previous
/// contents or the complete new ones, never a partial write.
///
/// The temporary file is created in the destination directory because `rename`
/// is only atomic within a filesystem, and is flushed before the rename so a
/// crash cannot leave the destination name pointing at unwritten data.
fn write_atomically(path: &Path, bytes: &[u8]) -> Result<()> {
    let Some(dir) = path.parent() else {
        return Err(Error::io(
            path,
            std::io::Error::other("not a writable file path"),
        ));
    };

    // `NamedTempFile` reserves a randomized name with exclusive creation. That
    // prevents both symlink attacks against a predictable temporary path and
    // collisions between concurrent writers in this process.
    let mut temp = tempfile::NamedTempFile::new_in(dir).map_err(|e| Error::io(dir, e))?;
    temp.write_all(bytes)
        .map_err(|e| Error::io(temp.path(), e))?;
    temp.as_file()
        .sync_all()
        .map_err(|e| Error::io(temp.path(), e))?;
    // `NamedTempFile` creates its file owner-only (0o600), and `persist` keeps
    // that mode. Everything written here is published — it is what a client
    // fetches from the channel's base URL — so open it up to the usual
    // world-readable mode a web server or perms-preserving sync expects.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        temp.as_file()
            .set_permissions(std::fs::Permissions::from_mode(0o644))
            .map_err(|e| Error::io(temp.path(), e))?;
    }
    let file = temp.persist(path).map_err(|e| Error::io(path, e.error))?;
    // Sync through the post-rename handle as a best-effort metadata flush on
    // platforms where `std` cannot open a directory for syncing.
    file.sync_all().map_err(|e| Error::io(path, e))?;

    sync_directory(dir)
}

/// Persist a directory-entry update after an atomic rename.
#[cfg(unix)]
fn sync_directory(path: &Path) -> Result<()> {
    std::fs::File::open(path)
        .and_then(|dir| dir.sync_all())
        .map_err(|e| Error::io(path, e))
}

/// Directory handles cannot be opened and synced portably through `std`.
#[cfg(not(unix))]
fn sync_directory(_path: &Path) -> Result<()> {
    Ok(())
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

    #[test]
    fn consistent_snapshot_target_naming_prefixes_the_file_name_only() {
        assert_eq!(
            target_published_name("manifest.json", "abc123"),
            "abc123.manifest.json"
        );
        assert_eq!(
            target_published_name("nested/dir/app.bin", "abc123"),
            "nested/dir/abc123.app.bin"
        );
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

    #[cfg(unix)]
    #[test]
    fn metadata_writes_do_not_follow_a_predictable_temporary_symlink() {
        let (dir, channel) = channel();
        let victim = dir.path().join("victim");
        std::fs::write(&victim, b"keep me").unwrap();

        let planted = channel
            .metadata_dir()
            .join(format!(".timestamp.json.{}.tmp", std::process::id()));
        std::os::unix::fs::symlink(&victim, &planted).unwrap();

        channel
            .write_metadata("timestamp.json", b"metadata")
            .unwrap();

        assert_eq!(std::fs::read(&victim).unwrap(), b"keep me");
        assert_eq!(
            std::fs::read(channel.metadata_dir().join("timestamp.json")).unwrap(),
            b"metadata"
        );
        assert!(planted.is_symlink());
    }

    #[cfg(unix)]
    #[test]
    fn published_files_are_world_readable() {
        use std::os::unix::fs::PermissionsExt;

        let (_dir, channel) = channel();
        channel
            .write_metadata("timestamp.json", b"metadata")
            .unwrap();
        channel
            .write_target("abc.manifest.json", b"target")
            .unwrap();

        for path in [
            channel.metadata_dir().join("timestamp.json"),
            channel.targets_dir().join("abc.manifest.json"),
        ] {
            let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o644, "{} must be servable", path.display());
        }
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
