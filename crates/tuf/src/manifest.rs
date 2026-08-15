//! The release-set manifest: the one file a channel distributes as a TUF
//! target.
//!
//! Everything else a release ships — binaries, archives, assets — is listed
//! here and authenticated transitively through this file's digest, never as an
//! individual TUF target. That digest is also the release's generation id, so
//! a byte-identical manifest always names the same generation.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::error::{Error, Result};

/// The TUF target path every channel publishes its manifest under.
pub const TARGET_PATH: &str = "manifest.json";

/// One distributable artifact of a release set.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Artifact {
    /// Stable artifact name, e.g. `retrovert-linux-x86_64`.
    pub name: String,
    /// Where the artifact is fetched from, relative to the release's base.
    pub target: String,
    /// Hex SHA-256 of the artifact's bytes.
    pub sha256: String,
    /// The artifact's length in bytes.
    pub size: u64,
    /// Additional fields defined by a compatible producer.
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

/// A release set: the artifacts of one publication, keyed by revision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    /// The source revision this release set was built from.
    pub revision: String,
    /// Human-readable version for display; carries no update semantics.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// The release set's artifacts.
    pub artifacts: Vec<Artifact>,
    /// Additional fields defined by a compatible producer.
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

impl Manifest {
    /// Parse and validate manifest `bytes`.
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        let manifest: Self =
            serde_json::from_slice(bytes).map_err(|e| Error::Manifest(e.to_string()))?;
        if manifest.revision.is_empty() {
            return Err(Error::Manifest("revision must not be empty".to_string()));
        }
        for artifact in &manifest.artifacts {
            if !is_hex_sha256(&artifact.sha256) {
                return Err(Error::Manifest(format!(
                    "artifact {:?} sha256 must be 64 lowercase hex characters",
                    artifact.name
                )));
            }
        }
        Ok(manifest)
    }
}

/// The generation id of the manifest serialized as `bytes`: its hex SHA-256.
///
/// Computed over the exact published bytes, so a byte-identical manifest
/// reproduces the same generation id.
#[must_use]
pub fn generation_id(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn is_hex_sha256(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest_json() -> serde_json::Value {
        serde_json::json!({
            "revision": "0123abc",
            "version": "1.2.0",
            "artifacts": [{
                "name": "app",
                "target": "app-linux-x86_64.tar.zst",
                "sha256": "a".repeat(64),
                "size": 42,
            }],
        })
    }

    #[test]
    fn a_valid_manifest_parses() {
        let manifest = Manifest::parse(&serde_json::to_vec(&manifest_json()).unwrap()).unwrap();
        assert_eq!(manifest.revision, "0123abc");
        assert_eq!(manifest.version.as_deref(), Some("1.2.0"));
        assert_eq!(manifest.artifacts.len(), 1);
        assert_eq!(manifest.artifacts[0].size, 42);
    }

    #[test]
    fn version_is_optional_but_revision_is_not() {
        let mut json = manifest_json();
        json.as_object_mut().unwrap().remove("version");
        assert!(Manifest::parse(&serde_json::to_vec(&json).unwrap()).is_ok());

        json.as_object_mut().unwrap().remove("revision");
        let err = Manifest::parse(&serde_json::to_vec(&json).unwrap()).unwrap_err();
        assert!(matches!(err, Error::Manifest(m) if m.contains("revision")));

        json["revision"] = "".into();
        let err = Manifest::parse(&serde_json::to_vec(&json).unwrap()).unwrap_err();
        assert!(matches!(err, Error::Manifest(m) if m.contains("revision")));
    }

    #[test]
    fn a_malformed_artifact_digest_is_rejected() {
        for bad in ["", "abc", &"A".repeat(64), &"g".repeat(64)] {
            let mut json = manifest_json();
            json["artifacts"][0]["sha256"] = bad.into();
            let err = Manifest::parse(&serde_json::to_vec(&json).unwrap()).unwrap_err();
            assert!(matches!(err, Error::Manifest(m) if m.contains("sha256")));
        }
    }

    #[test]
    fn unknown_fields_survive_a_round_trip() {
        let mut json = manifest_json();
        json["x-channel"] = "beta".into();
        json["artifacts"][0]["x-signature"] = "sig".into();

        let parsed = Manifest::parse(&serde_json::to_vec(&json).unwrap()).unwrap();
        assert_eq!(serde_json::to_value(parsed).unwrap(), json);
    }

    #[test]
    fn generation_id_is_the_sha256_of_the_bytes() {
        assert_eq!(
            generation_id(b"payload"),
            hex::encode(Sha256::digest(b"payload"))
        );
        assert_eq!(generation_id(b"payload"), generation_id(b"payload"));
        assert_ne!(generation_id(b"payload"), generation_id(b"payload2"));
    }
}
