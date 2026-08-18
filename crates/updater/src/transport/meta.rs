//! The `.meta` sidecar that makes an interrupted transfer resumable.

use std::fs;
use std::path::{Path, PathBuf};

const MAGIC: &[u8; 4] = b"DLMT";
const VERSION: u32 = 1;
/// `char[4]` + `u32` + `i64` + `i64` + `char[256]`, no tail padding.
const SIZE: usize = 280;
const ETAG_LEN: usize = 256;

/// What a partial transfer needs to pick up where it left off.
///
/// `etag` is the raw NUL-padded 256-byte field, so serialization round-trips.
#[derive(Clone)]
pub struct Meta {
    /// Length the completed file will have, as the server reported it.
    pub file_size: u64,
    /// Bytes already on disk.
    pub bytes_written: u64,
    /// The `ETag` the partial bytes came from, replayed as `If-Range`.
    pub etag: [u8; ETAG_LEN],
}

/// The sidecar path for the file at `path`.
#[must_use]
pub fn sidecar_path(path: &Path) -> PathBuf {
    let mut sidecar = path.as_os_str().to_os_string();
    sidecar.push(".meta");
    PathBuf::from(sidecar)
}

#[must_use]
pub fn serialize(meta: &Meta) -> [u8; SIZE] {
    let mut buf = [0u8; SIZE];
    buf[0..4].copy_from_slice(MAGIC);
    buf[4..8].copy_from_slice(&VERSION.to_le_bytes());
    buf[8..16].copy_from_slice(&meta.file_size.to_le_bytes());
    buf[16..24].copy_from_slice(&meta.bytes_written.to_le_bytes());
    buf[24..SIZE].copy_from_slice(&meta.etag);
    buf
}

#[must_use]
pub fn deserialize(data: &[u8]) -> Option<Meta> {
    // Shorter than one record is truncated or garbage.
    if data.len() < SIZE || &data[0..4] != MAGIC {
        return None;
    }
    let version = u32::from_le_bytes(data[4..8].try_into().ok()?);
    if version != VERSION {
        return None;
    }
    // The two size fields are `i64` on the wire; a negative is garbage.
    let file_size = read_size(&data[8..16])?;
    let bytes_written = read_size(&data[16..24])?;
    let mut etag = [0u8; ETAG_LEN];
    etag.copy_from_slice(&data[24..SIZE]);
    Some(Meta {
        file_size,
        bytes_written,
        etag,
    })
}

fn read_size(field: &[u8]) -> Option<u64> {
    u64::try_from(i64::from_le_bytes(field.try_into().ok()?)).ok()
}

#[must_use]
pub fn read(path: &Path) -> Option<Meta> {
    deserialize(&fs::read(sidecar_path(path)).ok()?)
}

pub fn write(path: &Path, meta: &Meta) -> bool {
    fs::write(sidecar_path(path), serialize(meta)).is_ok()
}

pub fn delete(path: &Path) {
    let _ = fs::remove_file(sidecar_path(path));
}

#[must_use]
pub fn pack_etag(etag: &str) -> [u8; ETAG_LEN] {
    let mut buf = [0u8; ETAG_LEN];
    let bytes = etag.as_bytes();
    let n = bytes.len().min(ETAG_LEN - 1);
    buf[..n].copy_from_slice(&bytes[..n]);
    buf
}

#[must_use]
pub fn unpack_etag(etag: &[u8; ETAG_LEN]) -> String {
    let end = etag.iter().position(|&c| c == 0).unwrap_or(ETAG_LEN);
    String::from_utf8_lossy(&etag[..end]).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_sidecar_sits_beside_the_file_it_describes() {
        assert_eq!(
            sidecar_path(Path::new("/cache/ab12")),
            Path::new("/cache/ab12.meta")
        );
    }

    #[test]
    fn a_record_round_trips() {
        let meta = Meta {
            file_size: 123_456,
            bytes_written: 65_536,
            etag: pack_etag("\"abc-123\""),
        };
        let bytes = serialize(&meta);
        assert_eq!(bytes.len(), SIZE);
        assert_eq!(&bytes[0..4], MAGIC);
        assert_eq!(u32::from_le_bytes(bytes[4..8].try_into().unwrap()), VERSION);

        let back = deserialize(&bytes).unwrap();
        assert_eq!(back.file_size, 123_456);
        assert_eq!(back.bytes_written, 65_536);
        assert_eq!(unpack_etag(&back.etag), "\"abc-123\"");
    }

    #[test]
    fn bad_magic_version_and_truncation_are_rejected() {
        let record = || {
            serialize(&Meta {
                file_size: 1,
                bytes_written: 1,
                etag: [0; ETAG_LEN],
            })
        };
        let mut bad_magic = record();
        bad_magic[0] = b'X';
        assert!(deserialize(&bad_magic).is_none());

        let mut bad_version = record();
        bad_version[4] = 2;
        assert!(deserialize(&bad_version).is_none());

        assert!(deserialize(b"x").is_none());
        assert!(deserialize(&record()[..SIZE - 1]).is_none());
    }

    #[test]
    fn a_negative_size_field_is_rejected() {
        let mut negative = serialize(&Meta {
            file_size: 1,
            bytes_written: 1,
            etag: [0; ETAG_LEN],
        });
        negative[8..16].copy_from_slice(&(-1i64).to_le_bytes());
        assert!(deserialize(&negative).is_none());
    }

    #[test]
    fn an_etag_packs_to_a_nul_terminated_field() {
        let long = "a".repeat(300);
        let packed = pack_etag(&long);
        assert_eq!(packed[255], 0);
        assert_eq!(unpack_etag(&packed), "a".repeat(255));
        // No NUL terminator (all 256 bytes used) reads back fully.
        let full = [b'b'; ETAG_LEN];
        assert_eq!(unpack_etag(&full), "b".repeat(256));
        assert_eq!(unpack_etag(&pack_etag("")), "");
    }
}
