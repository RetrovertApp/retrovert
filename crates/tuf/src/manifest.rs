//! The release-set manifest: the one file a channel distributes as a TUF
//! target.
//!
//! Everything else a release ships — binaries, archives, assets — is listed
//! here and authenticated transitively through this file's digest, never as an
//! individual TUF target. That digest is also the release's generation id, so
//! a byte-identical manifest always names the same generation.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::error::{Error, Result};

/// The TUF target path every channel publishes its manifest under.
pub const TARGET_PATH: &str = "manifest.json";

/// The manifest schema version this build reads and writes.
pub const SCHEMA_VERSION: u32 = 1;

/// One distributable artifact of a release set.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Artifact {
    /// Opaque artifact name; for plugins, the short name, e.g. `uade`.
    pub name: String,
    /// Opaque selector deciding which consumers want this artifact, e.g.
    /// `linux-x86_64`. Absent matches every consumer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    /// Where the artifact is fetched from, relative to the release's base.
    pub path: String,
    /// Hex SHA-256 of the artifact's bytes.
    pub sha256: String,
    /// The artifact's length in bytes.
    pub size: u64,
    /// The source commit this artifact was built from — the per-artifact
    /// corresponding-source pointer the licensing policy publishes.
    pub revision: String,
    /// Human-readable version for display; carries no update semantics.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

/// A release set: the artifacts of one publication.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    /// Always [`SCHEMA_VERSION`].
    pub schema: u32,
    /// The channel's monotonic release-set number, matching its `<channel>/vN`
    /// tag.
    pub version: u64,
    /// The aggregate repository commit this release set was gathered from.
    pub source_revision: String,
    /// When this set was published, RFC 3339 in UTC.
    pub published: String,
    /// The release set's artifacts.
    pub artifacts: Vec<Artifact>,
}

/// Reads the schema version without committing to the rest of the document.
#[derive(Deserialize)]
struct SchemaProbe {
    schema: u32,
}

impl Manifest {
    /// Parse and validate manifest `bytes`.
    ///
    /// This is the validating entry point; deserializing a [`Manifest`]
    /// directly bypasses these checks.
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        // Read the schema first so a future document reports the version it
        // is, rather than whichever field this build happened to miss.
        let probe: SchemaProbe =
            serde_json::from_slice(bytes).map_err(|e| Error::Manifest(e.to_string()))?;
        if probe.schema != SCHEMA_VERSION {
            return Err(Error::Manifest(format!(
                "schema {} is not readable by this build, which reads schema {SCHEMA_VERSION}",
                probe.schema
            )));
        }

        let manifest: Self =
            serde_json::from_slice(bytes).map_err(|e| Error::Manifest(e.to_string()))?;
        if manifest.version == 0 {
            return Err(Error::Manifest(
                "version must be a positive release-set number".to_string(),
            ));
        }
        if manifest.source_revision.is_empty() {
            return Err(Error::Manifest(
                "source_revision must not be empty".to_string(),
            ));
        }
        if manifest.published.parse::<jiff::Timestamp>().is_err() {
            return Err(Error::Manifest(format!(
                "published must be an RFC 3339 timestamp, got {:?}",
                manifest.published
            )));
        }

        // A plugin ships once per target, so name alone is not an identity —
        // only the pair has to be unique.
        let mut seen = BTreeSet::new();
        for artifact in &manifest.artifacts {
            if artifact.name.is_empty() {
                return Err(Error::Manifest(
                    "artifact name must not be empty".to_string(),
                ));
            }
            if !seen.insert((artifact.name.as_str(), artifact.target.as_deref())) {
                return Err(Error::Manifest(format!(
                    "duplicate artifact {:?} for target {:?}",
                    artifact.name, artifact.target
                )));
            }
            if artifact.target.as_ref().is_some_and(String::is_empty) {
                return Err(Error::Manifest(format!(
                    "artifact {:?} target must not be empty; omit it to match every consumer",
                    artifact.name
                )));
            }
            if artifact.revision.is_empty() {
                return Err(Error::Manifest(format!(
                    "artifact {:?} revision must not be empty",
                    artifact.name
                )));
            }
            if !is_hex_sha256(&artifact.sha256) {
                return Err(Error::Manifest(format!(
                    "artifact {:?} sha256 must be 64 lowercase hex characters",
                    artifact.name
                )));
            }
            if !is_clean_relative_path(&artifact.path) {
                return Err(Error::Manifest(format!(
                    "artifact {:?} path must be a clean relative path, got {:?}",
                    artifact.name, artifact.path
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

/// Whether `s` is a `/`-separated relative path that stays under the release's
/// base: no absolute form, no `.`/`..` components, no empty components, and no
/// backslashes that a Windows path resolver could reinterpret.
fn is_clean_relative_path(s: &str) -> bool {
    !s.is_empty()
        && !s.contains('\\')
        && s.split('/')
            .all(|part| !part.is_empty() && part != "." && part != "..")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest_json() -> serde_json::Value {
        serde_json::json!({
            "schema": 1,
            "version": 3,
            "source_revision": "0123abc",
            "published": "2026-08-15T12:00:00Z",
            "artifacts": [{
                "name": "spu",
                "target": "linux-x86_64",
                "path": "spu-linux-x86_64.tar.zst",
                "sha256": "a".repeat(64),
                "size": 42,
                "revision": "def4567",
            }],
        })
    }

    fn parse(json: &serde_json::Value) -> Result<Manifest> {
        Manifest::parse(&serde_json::to_vec(json).unwrap())
    }

    #[test]
    fn a_valid_manifest_parses() {
        let manifest = parse(&manifest_json()).unwrap();
        assert_eq!(manifest.version, 3);
        assert_eq!(manifest.source_revision, "0123abc");
        assert_eq!(manifest.artifacts.len(), 1);

        let artifact = &manifest.artifacts[0];
        assert_eq!(artifact.name, "spu");
        assert_eq!(artifact.target.as_deref(), Some("linux-x86_64"));
        assert_eq!(artifact.path, "spu-linux-x86_64.tar.zst");
        assert_eq!(artifact.revision, "def4567");
        assert_eq!(artifact.size, 42);
        assert_eq!(artifact.version, None);
    }

    #[test]
    fn a_manifest_from_another_schema_names_its_own_version() {
        let mut json = manifest_json();
        json["schema"] = 2.into();
        // The rest of the document is unreadable too, so the schema check has
        // to win before any field does.
        json.as_object_mut().unwrap().remove("source_revision");

        let err = parse(&json).unwrap_err();
        assert!(matches!(err, Error::Manifest(m) if m.contains("schema 2")));

        json = manifest_json();
        json.as_object_mut().unwrap().remove("schema");
        let err = parse(&json).unwrap_err();
        assert!(matches!(err, Error::Manifest(m) if m.contains("schema")));
    }

    #[test]
    fn a_schema_1_document_carrying_unknown_fields_still_parses() {
        let mut json = manifest_json();
        json["channel_note"] = "written by a later producer".into();
        json["artifacts"][0]["signature_path"] = "spu-linux-x86_64.tar.zst.sig".into();

        assert!(parse(&json).is_ok(), "schema 1 extends, never replaces");
    }

    #[test]
    fn mandatory_release_set_fields_are_enforced() {
        for field in ["version", "source_revision", "published", "artifacts"] {
            let mut json = manifest_json();
            json.as_object_mut().unwrap().remove(field);
            assert!(parse(&json).is_err(), "{field} must be mandatory");
        }

        let mut json = manifest_json();
        json["version"] = 0.into();
        let err = parse(&json).unwrap_err();
        assert!(matches!(err, Error::Manifest(m) if m.contains("version")));

        json = manifest_json();
        json["source_revision"] = "".into();
        let err = parse(&json).unwrap_err();
        assert!(matches!(err, Error::Manifest(m) if m.contains("source_revision")));
    }

    #[test]
    fn published_must_be_a_timestamp() {
        for bad in ["", "yesterday", "2026-08-15"] {
            let mut json = manifest_json();
            json["published"] = bad.into();
            let err = parse(&json).unwrap_err();
            assert!(
                matches!(err, Error::Manifest(ref m) if m.contains("published")),
                "{bad:?} must be rejected"
            );
        }
    }

    #[test]
    fn an_artifact_without_a_target_matches_every_consumer() {
        let mut json = manifest_json();
        json["artifacts"][0]
            .as_object_mut()
            .unwrap()
            .remove("target");

        let manifest = parse(&json).unwrap();
        assert_eq!(manifest.artifacts[0].target, None);

        json["artifacts"][0]["target"] = "".into();
        let err = parse(&json).unwrap_err();
        assert!(matches!(err, Error::Manifest(m) if m.contains("target")));
    }

    #[test]
    fn one_name_may_ship_once_per_target() {
        let mut json = manifest_json();
        let mut other = json["artifacts"][0].clone();
        other["target"] = "windows-x86_64".into();
        other["path"] = "spu-windows-x86_64.tar.zst".into();
        json["artifacts"].as_array_mut().unwrap().push(other);
        assert_eq!(parse(&json).unwrap().artifacts.len(), 2);

        let repeated = json["artifacts"][1].clone();
        json["artifacts"].as_array_mut().unwrap().push(repeated);
        let err = parse(&json).unwrap_err();
        assert!(matches!(err, Error::Manifest(m) if m.contains("duplicate")));
    }

    #[test]
    fn an_artifact_revision_is_mandatory() {
        let mut json = manifest_json();
        json["artifacts"][0]["revision"] = "".into();
        let err = parse(&json).unwrap_err();
        assert!(matches!(err, Error::Manifest(m) if m.contains("revision")));

        json["artifacts"][0]
            .as_object_mut()
            .unwrap()
            .remove("revision");
        assert!(parse(&json).is_err());
    }

    #[test]
    fn an_artifact_display_version_is_optional() {
        let mut json = manifest_json();
        assert_eq!(parse(&json).unwrap().artifacts[0].version, None);

        json["artifacts"][0]["version"] = "1.2.0".into();
        assert_eq!(
            parse(&json).unwrap().artifacts[0].version.as_deref(),
            Some("1.2.0")
        );
    }

    #[test]
    fn an_artifact_name_must_not_be_empty() {
        let mut json = manifest_json();
        json["artifacts"][0]["name"] = "".into();
        let repeated = json["artifacts"][0].clone();
        json["artifacts"].as_array_mut().unwrap().push(repeated);

        // Two nameless entries are duplicates as well; the name check has to win.
        let err = parse(&json).unwrap_err();
        assert!(matches!(err, Error::Manifest(m) if m.contains("name")));
    }

    #[test]
    fn a_malformed_artifact_digest_is_rejected() {
        for bad in ["", "abc", &"A".repeat(64), &"g".repeat(64)] {
            let mut json = manifest_json();
            json["artifacts"][0]["sha256"] = bad.into();
            let err = parse(&json).unwrap_err();
            assert!(matches!(err, Error::Manifest(m) if m.contains("sha256")));
        }
    }

    #[test]
    fn a_traversing_or_absolute_artifact_path_is_rejected() {
        for bad in [
            "",
            "/etc/passwd",
            "../outside",
            "dir/../outside",
            "dir/..",
            "./app",
            "dir//app",
            "dir/",
            "dir\\app",
        ] {
            let mut json = manifest_json();
            json["artifacts"][0]["path"] = bad.into();
            let err = parse(&json).unwrap_err();
            assert!(
                matches!(err, Error::Manifest(ref m) if m.contains("path")),
                "{bad:?} must be rejected"
            );
        }

        let mut json = manifest_json();
        json["artifacts"][0]["path"] = "nested/dir/app.bin".into();
        assert!(parse(&json).is_ok());
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
