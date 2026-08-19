//! What a completed transfer has to prove before it counts as complete.
//!
//! Three gates, in order: the bytes as they streamed past hashed to the digest
//! the request named, the file is the length the request named, and the file
//! re-read from disk still hashes to that digest. The last one is not
//! redundant — it catches a write that never reached the platter.

use std::fs::File;
use std::io::Read;
use std::path::Path;

use sha2::{Digest, Sha256};

use crate::transport::{self, ArtifactDigest};

/// The read size used for hashing, both in flight and from disk.
pub(super) const CHUNK_SIZE: usize = 64 * 1024;

/// Why a queued transfer failed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum Failure {
    /// The bytes that streamed past did not hash to the requested digest.
    #[error("hashed to {actual} in flight, not the digest the request named")]
    Digest {
        /// What the transferred bytes hashed to.
        actual: String,
    },

    /// The completed file did not hash to the requested digest when re-read.
    #[error("re-read from disk as {actual}, not the digest the request named")]
    DiskDigest {
        /// What the file on disk hashed to.
        actual: String,
    },

    /// The completed file was not the length the request named.
    #[error("completed at {actual} bytes, not the {expected} the request named")]
    Size {
        /// The length the request named.
        expected: u64,
        /// The length that arrived.
        actual: u64,
    },

    /// The completed file could not be read back for verification.
    #[error("the completed file could not be re-read for verification")]
    Unreadable,

    /// The transfer itself failed.
    #[error("{0}")]
    Transfer(transport::Failure),

    /// The transfer could not be started at all.
    #[error("the transfer could not be started: {0}")]
    Start(String),

    /// The worker panicked partway through the transfer.
    #[error("the transfer panicked")]
    Panicked,
}

/// Check a completed transfer against everything its request promised.
pub(super) fn validate(
    path: &Path,
    digest: &ArtifactDigest,
    expected_size: Option<u64>,
    in_flight: Sha256,
    downloaded: u64,
) -> Result<(), Failure> {
    let hashed: [u8; 32] = in_flight.finalize().into();
    if &hashed != digest.as_bytes() {
        return Err(Failure::Digest {
            actual: hex::encode(hashed),
        });
    }

    if let Some(expected) = expected_size {
        if downloaded != expected {
            return Err(Failure::Size {
                expected,
                actual: downloaded,
            });
        }
    }

    verify_on_disk(path, digest)
}

/// Check that the file at `path` reads back as `digest`.
///
/// The last of [`validate`]'s three gates on its own, for a caller holding a
/// file it did not stream: publication copies a validated artifact out of the
/// cache, and the copy has to prove the same thing the original did.
pub(crate) fn verify_on_disk(path: &Path, digest: &ArtifactDigest) -> Result<(), Failure> {
    match hash_file(path) {
        None => Err(Failure::Unreadable),
        Some(on_disk) if &on_disk != digest.as_bytes() => Err(Failure::DiskDigest {
            actual: hex::encode(on_disk),
        }),
        Some(_) => Ok(()),
    }
}

/// Feed the first `len` bytes of the file at `path` into `digest`.
///
/// A resumed transfer streams only its tail, so the bytes already on disk have
/// to go through the digest as well for the in-flight check to cover the whole
/// artifact. Reports false when the file is shorter than `len`.
pub(super) fn hash_prefix(path: &Path, len: u64, digest: &mut Sha256) -> bool {
    let Ok(file) = File::open(path) else {
        return false;
    };
    let mut reader = file.take(len);
    let mut buffer = vec![0u8; CHUNK_SIZE];
    let mut total = 0u64;
    loop {
        let Ok(read) = reader.read(&mut buffer) else {
            return false;
        };
        if read == 0 {
            break;
        }
        total += read as u64;
        digest.update(&buffer[..read]);
    }
    total == len
}

/// Hash the file at `path`, or `None` when it cannot be read or holds nothing.
///
/// Read a chunk at a time rather than whole: an artifact can be large, and the
/// digest only ever sees one [`CHUNK_SIZE`] window this way.
///
/// An empty file is `None`, not the digest of nothing. That digest is a real
/// value and would be compared against the expected one on its merits, quietly
/// turning a truncated transfer into a digest mismatch instead of an unreadable
/// file.
pub(super) fn hash_file(path: &Path) -> Option<[u8; 32]> {
    let mut file = File::open(path).ok()?;
    let mut digest = Sha256::new();
    let mut buffer = vec![0u8; CHUNK_SIZE];
    let mut total = 0u64;
    loop {
        let read = file.read(&mut buffer).ok()?;
        if read == 0 {
            break;
        }
        total += read as u64;
        digest.update(&buffer[..read]);
    }
    (total > 0).then(|| digest.finalize().into())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    /// The published SHA-256 of "abc", the vector this check has always used.
    const ABC: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";

    fn hashed(bytes: &[u8]) -> Sha256 {
        let mut digest = Sha256::new();
        digest.update(bytes);
        digest
    }

    #[test]
    fn hash_file_digests_the_bytes_on_disk() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("artifact");
        fs::write(&path, b"abc").unwrap();
        assert_eq!(hash_file(&path).map(hex::encode).as_deref(), Some(ABC));
    }

    #[test]
    fn hash_file_is_none_for_a_missing_path() {
        let dir = tempfile::tempdir().unwrap();
        assert!(hash_file(&dir.path().join("absent")).is_none());
    }

    /// An empty file reads successfully — it just has no bytes. Verification
    /// must report that as "cannot be hashed" rather than answering with the
    /// digest of nothing, which is a real digest and would be compared against
    /// the expected one on its merits.
    #[test]
    fn hash_file_is_none_for_an_empty_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("artifact");
        fs::write(&path, b"").unwrap();
        assert!(hash_file(&path).is_none());
    }

    #[test]
    fn a_prefix_hashed_off_disk_continues_into_the_streamed_tail() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("artifact");
        fs::write(&path, b"ab").unwrap();

        // "ab" off disk plus "c" streamed hashes as if "abc" had streamed whole.
        let mut digest = Sha256::new();
        assert!(hash_prefix(&path, 2, &mut digest));
        digest.update(b"c");
        assert_eq!(hex::encode(digest.finalize()), ABC);
    }

    #[test]
    fn a_prefix_longer_than_the_file_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("artifact");
        fs::write(&path, b"ab").unwrap();

        assert!(!hash_prefix(&path, 3, &mut Sha256::new()));
        assert!(!hash_prefix(
            &dir.path().join("absent"),
            1,
            &mut Sha256::new()
        ));
    }

    #[test]
    fn matching_bytes_size_and_disk_contents_validate() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("artifact");
        fs::write(&path, b"abc").unwrap();
        let digest = ArtifactDigest::from_hex(ABC).unwrap();

        assert_eq!(validate(&path, &digest, Some(3), hashed(b"abc"), 3), Ok(()));
        // No size named is no size gate.
        assert_eq!(validate(&path, &digest, None, hashed(b"abc"), 3), Ok(()));
    }

    #[test]
    fn bytes_that_hash_wrong_in_flight_are_refused_before_the_disk_is_read() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("absent");
        let digest = ArtifactDigest::from_hex(ABC).unwrap();

        // The file does not exist, so reaching the disk check would report
        // Unreadable instead.
        let failure = validate(&path, &digest, None, hashed(b"xyz"), 3).unwrap_err();
        assert!(matches!(failure, Failure::Digest { .. }));
    }

    #[test]
    fn a_size_the_request_did_not_name_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("artifact");
        fs::write(&path, b"abc").unwrap();
        let digest = ArtifactDigest::from_hex(ABC).unwrap();

        assert_eq!(
            validate(&path, &digest, Some(4), hashed(b"abc"), 3),
            Err(Failure::Size {
                expected: 4,
                actual: 3
            })
        );
    }

    #[test]
    fn a_file_that_disagrees_with_the_bytes_that_streamed_past_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("artifact");
        let digest = ArtifactDigest::from_hex(ABC).unwrap();

        // Hashed "abc" in flight, but something else landed on disk.
        fs::write(&path, b"xyz").unwrap();
        let failure = validate(&path, &digest, None, hashed(b"abc"), 3).unwrap_err();
        assert!(matches!(failure, Failure::DiskDigest { .. }));

        // Nothing landed at all.
        fs::write(&path, b"").unwrap();
        assert_eq!(
            validate(&path, &digest, None, hashed(b"abc"), 3),
            Err(Failure::Unreadable)
        );
    }
}
