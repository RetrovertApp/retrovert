//! The expected SHA-256 of an artifact.

use std::fmt;

use super::error::Error;

const DIGEST_LEN: usize = 32;

/// The expected SHA-256 of an artifact's bytes, and the key of its cache entry.
///
/// Parsing here is what stops a manifest-supplied digest reaching the
/// filesystem as an arbitrary path component.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ArtifactDigest([u8; DIGEST_LEN]);

impl ArtifactDigest {
    /// Parse 64 hex characters of either case.
    pub fn from_hex(hex_str: &str) -> Result<Self, Error> {
        let mut bytes = [0u8; DIGEST_LEN];
        hex::decode_to_slice(hex_str, &mut bytes)
            .map_err(|_| Error::Digest(hex_str.to_string()))?;
        Ok(Self(bytes))
    }

    /// The raw digest bytes.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8; DIGEST_LEN] {
        &self.0
    }
}

impl fmt::Display for ArtifactDigest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for byte in self.0 {
            write!(f, "{byte:02x}")?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HEX: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

    #[test]
    fn a_digest_round_trips_through_lowercase_hex() {
        let digest = ArtifactDigest::from_hex(HEX).unwrap();
        assert_eq!(digest.to_string(), HEX);
        assert_eq!(digest.as_bytes()[0], 0xe3);
    }

    #[test]
    fn uppercase_hex_parses_to_the_same_digest() {
        assert_eq!(
            ArtifactDigest::from_hex(&HEX.to_uppercase()).unwrap(),
            ArtifactDigest::from_hex(HEX).unwrap()
        );
    }

    #[test]
    fn anything_but_64_hex_characters_is_rejected() {
        for bad in ["", &HEX[..63], &format!("{HEX}00"), &HEX.replace('e', "z")] {
            assert!(ArtifactDigest::from_hex(bad).is_err(), "accepted {bad:?}");
        }
    }
}
