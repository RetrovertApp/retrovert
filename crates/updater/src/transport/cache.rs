//! The download cache, owned by one transport instance and keyed by digest.
//!
//! An entry is `<root>/<hex digest>` with its resume sidecar at
//! `<root>/<hex digest>.meta`. Keying on the artifact's expected digest rather
//! than its URL makes evicting a poisoned entry exact, and leaves the cache
//! intact when a mirror changes.

use std::fs;
use std::path::{Path, PathBuf};

use super::digest::ArtifactDigest;
use super::meta;

/// A directory of digest-keyed download entries.
#[derive(Debug, Clone)]
pub struct Cache {
    root: PathBuf,
}

impl Cache {
    /// A cache rooted at `root`, which is created when the first entry needs it.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// Where the artifact with this digest is cached.
    #[must_use]
    pub fn path_for(&self, digest: &ArtifactDigest) -> PathBuf {
        self.root.join(digest.to_string())
    }

    /// Whether the entry is present and complete.
    #[must_use]
    pub fn is_complete(&self, digest: &ArtifactDigest) -> bool {
        is_complete_at(&self.path_for(digest))
    }

    /// Whether an interrupted transfer left something to resume.
    #[cfg_attr(not(test), allow(dead_code))]
    #[must_use]
    pub fn has_partial(&self, digest: &ArtifactDigest) -> bool {
        meta::read(&self.path_for(digest)).is_some()
    }

    /// Drop the entry and its sidecar.
    pub fn evict(&self, digest: &ArtifactDigest) {
        let path = self.path_for(digest);
        meta::delete(&path);
        let _ = fs::remove_file(&path);
    }
}

/// Complete means the file exists, is non-empty, and its sidecar says every
/// byte arrived.
pub(super) fn is_complete_at(path: &Path) -> bool {
    let Ok(metadata) = fs::metadata(path) else {
        return false;
    };
    if !metadata.is_file() || metadata.len() == 0 {
        return false;
    }
    match meta::read(path) {
        Some(m) => m.bytes_written == m.file_size,
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DIGEST_A: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
    const DIGEST_B: &str = "0000000000000000000000000000000000000000000000000000000000000001";

    fn digest(hex_str: &str) -> ArtifactDigest {
        ArtifactDigest::from_hex(hex_str).unwrap()
    }

    #[test]
    fn an_entry_is_named_for_its_digest_alone() {
        let cache = Cache::new("/cache");
        assert_eq!(
            cache.path_for(&digest(DIGEST_A)),
            Path::new("/cache").join(DIGEST_A)
        );
        assert_ne!(
            cache.path_for(&digest(DIGEST_A)),
            cache.path_for(&digest(DIGEST_B))
        );
        assert_eq!(
            cache.path_for(&digest(&DIGEST_A.to_uppercase())),
            cache.path_for(&digest(DIGEST_A))
        );
    }

    #[test]
    fn completeness_follows_the_sidecar_not_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::new(dir.path());
        let digest = digest(DIGEST_A);
        let path = cache.path_for(&digest);

        fs::write(&path, b"partial data").unwrap();
        assert!(!cache.is_complete(&digest));
        assert!(!cache.has_partial(&digest));

        meta::write(
            &path,
            &meta::Meta {
                file_size: 1000,
                bytes_written: 12,
                etag: [0; 256],
            },
        );
        assert!(cache.has_partial(&digest));
        assert!(!cache.is_complete(&digest));

        meta::write(
            &path,
            &meta::Meta {
                file_size: 12,
                bytes_written: 12,
                etag: [0; 256],
            },
        );
        assert!(cache.is_complete(&digest));

        // An empty file is never complete, whatever its sidecar claims.
        fs::write(&path, b"").unwrap();
        meta::write(
            &path,
            &meta::Meta {
                file_size: 0,
                bytes_written: 0,
                etag: [0; 256],
            },
        );
        assert!(!cache.is_complete(&digest));
    }

    #[test]
    fn eviction_removes_both_halves_of_an_entry() {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::new(dir.path());
        let digest = digest(DIGEST_A);
        let path = cache.path_for(&digest);

        fs::write(&path, b"poison").unwrap();
        meta::write(
            &path,
            &meta::Meta {
                file_size: 6,
                bytes_written: 6,
                etag: [0; 256],
            },
        );

        cache.evict(&digest);
        assert!(!path.exists());
        assert!(!meta::sidecar_path(&path).exists());
        assert!(!cache.is_complete(&digest));
        assert!(!cache.has_partial(&digest));
    }
}
