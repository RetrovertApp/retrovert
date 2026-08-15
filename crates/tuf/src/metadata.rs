//! The TUF metadata model: the signed envelope and the four top-level roles.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::canonical;
use crate::error::Result;
use crate::key::{KeyPair, PublicKey};
use crate::policy;

/// The four top-level TUF roles.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum RoleName {
    /// The trust anchor, held offline.
    Root,
    /// The inventory of distributable targets.
    Targets,
    /// The pin on every targets metadata version.
    Snapshot,
    /// The pin on the current snapshot version.
    Timestamp,
}

impl RoleName {
    /// Every top-level role, in signing order: the roles a parent pins are
    /// signed before the parent, so `timestamp` is last and is therefore the
    /// atomic commit point of a publish.
    pub const ALL: [RoleName; 4] = [
        RoleName::Root,
        RoleName::Targets,
        RoleName::Snapshot,
        RoleName::Timestamp,
    ];

    /// The role's TUF name, which is also its metadata file stem.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            RoleName::Root => "root",
            RoleName::Targets => "targets",
            RoleName::Snapshot => "snapshot",
            RoleName::Timestamp => "timestamp",
        }
    }

    /// The unversioned metadata file name, e.g. `snapshot.json`.
    #[must_use]
    pub fn file_name(self) -> String {
        format!("{}.json", self.as_str())
    }
}

impl std::fmt::Display for RoleName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A role payload that can be wrapped in a [`Signed`] envelope.
pub trait Role: Serialize {
    /// Which role this payload is.
    const NAME: RoleName;
}

/// A single signature over a metadata file's canonical `signed` bytes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Signature {
    /// The signing key's TUF key ID.
    pub keyid: String,
    /// Hex-encoded raw Ed25519 signature.
    pub sig: String,
}

/// A metadata file: a role payload plus the signatures over it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Signed<T> {
    /// Signatures over the canonical bytes of `signed`, ordered by key ID.
    pub signatures: Vec<Signature>,
    /// The role payload.
    pub signed: T,
}

impl<T: Role> Signed<T> {
    /// Sign `payload` with every key in `keys`.
    pub fn new(payload: T, keys: &[&KeyPair]) -> Result<Self> {
        let canonical = canonical::to_bytes(&payload)?;
        let mut signatures = keys
            .iter()
            .map(|key| {
                Ok(Signature {
                    keyid: key.key_id()?,
                    sig: key.sign_hex(&canonical),
                })
            })
            .collect::<Result<Vec<_>>>()?;
        signatures.sort_by(|a, b| a.keyid.cmp(&b.keyid));
        Ok(Self {
            signatures,
            signed: payload,
        })
    }

    /// Render as the pretty-printed JSON published to a channel.
    ///
    /// Formatting is free to differ from the canonical bytes: verifiers
    /// re-canonicalize the parsed `signed` object before checking signatures.
    pub fn to_json(&self) -> Result<Vec<u8>> {
        let mut bytes = serde_json::to_vec_pretty(self)?;
        bytes.push(b'\n');
        Ok(bytes)
    }
}

/// The keys and signature threshold authorized for a role.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoleKeys {
    /// TUF key IDs authorized to sign for this role.
    pub keyids: Vec<String>,
    /// Number of distinct valid signatures required.
    pub threshold: u32,
}

/// The `root` role: the trust anchor that delegates to all other roles.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Root {
    #[serde(rename = "_type")]
    type_: String,
    spec_version: String,
    /// Monotonically increasing version number.
    pub version: u64,
    /// Expiry, RFC 3339 in UTC.
    pub expires: String,
    /// Whether metadata and targets carry version- and hash-prefixed names.
    pub consistent_snapshot: bool,
    /// Every key referenced by a role, indexed by key ID.
    pub keys: BTreeMap<String, PublicKey>,
    /// Role name to its authorized keys and threshold.
    pub roles: BTreeMap<String, RoleKeys>,
}

impl Root {
    /// Build a root that authorizes exactly one key per role at threshold 1.
    pub fn single_key_per_role(
        version: u64,
        expires: String,
        role_keys: &[(RoleName, PublicKey)],
    ) -> Result<Self> {
        let mut keys = BTreeMap::new();
        let mut roles = BTreeMap::new();
        for (role, public) in role_keys {
            let key_id = public.key_id()?;
            roles.insert(
                role.as_str().to_string(),
                RoleKeys {
                    keyids: vec![key_id.clone()],
                    threshold: 1,
                },
            );
            keys.insert(key_id, public.clone());
        }
        Ok(Self {
            type_: RoleName::Root.as_str().to_string(),
            spec_version: policy::SPEC_VERSION.to_string(),
            version,
            expires,
            consistent_snapshot: true,
            keys,
            roles,
        })
    }
}

impl Role for Root {
    const NAME: RoleName = RoleName::Root;
}

/// A reference to another metadata file, as recorded by a parent role.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MetaFile {
    /// The referenced file's version.
    pub version: u64,
    /// Its exact length in bytes.
    pub length: u64,
    /// Its hashes, algorithm to hex digest.
    pub hashes: BTreeMap<String, String>,
}

impl MetaFile {
    /// Pin `bytes` — the exact published file — at `version`.
    #[must_use]
    pub fn pinning(version: u64, bytes: &[u8]) -> Self {
        Self {
            version,
            length: bytes.len() as u64,
            hashes: sha256_map(bytes),
        }
    }
}

/// A distributable target file's metadata.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TargetFile {
    /// The target's length in bytes.
    pub length: u64,
    /// The target's hashes, algorithm to hex digest.
    pub hashes: BTreeMap<String, String>,
    /// Opaque application metadata.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub custom: Option<serde_json::Value>,
}

/// The `targets` role: the inventory of distributable files.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Targets {
    #[serde(rename = "_type")]
    type_: String,
    spec_version: String,
    /// Monotonically increasing version number.
    pub version: u64,
    /// Expiry, RFC 3339 in UTC.
    pub expires: String,
    /// Target path to its metadata.
    pub targets: BTreeMap<String, TargetFile>,
}

impl Targets {
    /// Build a targets role listing `targets`.
    #[must_use]
    pub fn new(version: u64, expires: String, targets: BTreeMap<String, TargetFile>) -> Self {
        Self {
            type_: RoleName::Targets.as_str().to_string(),
            spec_version: policy::SPEC_VERSION.to_string(),
            version,
            expires,
            targets,
        }
    }
}

impl Role for Targets {
    const NAME: RoleName = RoleName::Targets;
}

/// The `snapshot` role: pins the version of every targets metadata file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Snapshot {
    #[serde(rename = "_type")]
    type_: String,
    spec_version: String,
    /// Monotonically increasing version number.
    pub version: u64,
    /// Expiry, RFC 3339 in UTC.
    pub expires: String,
    /// Targets metadata file name to its pin.
    pub meta: BTreeMap<String, MetaFile>,
}

impl Snapshot {
    /// Build a snapshot pinning `meta`.
    #[must_use]
    pub fn new(version: u64, expires: String, meta: BTreeMap<String, MetaFile>) -> Self {
        Self {
            type_: RoleName::Snapshot.as_str().to_string(),
            spec_version: policy::SPEC_VERSION.to_string(),
            version,
            expires,
            meta,
        }
    }
}

impl Role for Snapshot {
    const NAME: RoleName = RoleName::Snapshot;
}

/// The `timestamp` role: pins the current snapshot version.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Timestamp {
    #[serde(rename = "_type")]
    type_: String,
    spec_version: String,
    /// Monotonically increasing version number.
    pub version: u64,
    /// Expiry, RFC 3339 in UTC.
    pub expires: String,
    /// A single entry, `snapshot.json`.
    pub meta: BTreeMap<String, MetaFile>,
}

impl Timestamp {
    /// Build a timestamp pinning `snapshot`.
    #[must_use]
    pub fn new(version: u64, expires: String, snapshot: MetaFile) -> Self {
        Self {
            type_: RoleName::Timestamp.as_str().to_string(),
            spec_version: policy::SPEC_VERSION.to_string(),
            version,
            expires,
            meta: BTreeMap::from([(RoleName::Snapshot.file_name(), snapshot)]),
        }
    }
}

impl Role for Timestamp {
    const NAME: RoleName = RoleName::Timestamp;
}

/// The `{"sha256": "<hex>"}` hash map TUF records for a file.
#[must_use]
pub fn sha256_map(bytes: &[u8]) -> BTreeMap<String, String> {
    BTreeMap::from([("sha256".to_string(), hex::encode(Sha256::digest(bytes)))])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signatures_are_ordered_by_key_id() {
        let keys: Vec<KeyPair> = (0..4u8).map(|i| KeyPair::from_seed(&[i; 32])).collect();
        let refs: Vec<&KeyPair> = keys.iter().collect();
        let targets = Targets::new(1, "2027-01-01T00:00:00Z".to_string(), BTreeMap::new());
        let signed = Signed::new(targets, &refs).unwrap();

        let ids: Vec<&str> = signed.signatures.iter().map(|s| s.keyid.as_str()).collect();
        let mut sorted = ids.clone();
        sorted.sort_unstable();
        assert_eq!(ids, sorted);
        assert_eq!(ids.len(), 4);
    }

    #[test]
    fn published_json_recanonicalizes_to_the_signed_bytes() {
        let key = KeyPair::from_seed(&[1u8; 32]);
        let targets = Targets::new(1, "2027-01-01T00:00:00Z".to_string(), BTreeMap::new());
        let expected = canonical::to_bytes(&targets).unwrap();
        let signed = Signed::new(targets, &[&key]).unwrap();

        let parsed: serde_json::Value = serde_json::from_slice(&signed.to_json().unwrap()).unwrap();
        assert_eq!(canonical::to_bytes(&parsed["signed"]).unwrap(), expected);
    }

    #[test]
    fn root_authorizes_one_key_per_role() {
        let keys: Vec<(RoleName, KeyPair)> = RoleName::ALL
            .iter()
            .enumerate()
            .map(|(i, r)| (*r, KeyPair::from_seed(&[u8::try_from(i).unwrap(); 32])))
            .collect();
        let role_keys: Vec<(RoleName, PublicKey)> =
            keys.iter().map(|(r, k)| (*r, k.public())).collect();
        let root =
            Root::single_key_per_role(1, "2027-01-01T00:00:00Z".to_string(), &role_keys).unwrap();

        assert_eq!(root.keys.len(), 4);
        assert_eq!(root.roles.len(), 4);
        assert!(root.consistent_snapshot);
        for role in RoleName::ALL {
            let entry = &root.roles[role.as_str()];
            assert_eq!(entry.threshold, 1);
            assert_eq!(entry.keyids.len(), 1);
            assert!(root.keys.contains_key(&entry.keyids[0]));
        }
    }
}
