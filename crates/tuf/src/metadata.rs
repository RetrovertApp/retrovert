//! The TUF metadata model: the signed envelope and the four top-level roles.
//!
//! Every type here preserves unknown fields on deserialization. TUF permits
//! compatible producers to extend metadata objects, and those fields must
//! remain present when signed bytes are canonicalized or reserialized.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::canonical;
use crate::error::{Error, Result};
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
    /// Through `pad`, so a caller aligning a column of role names gets the
    /// width it asked for.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.pad(self.as_str())
    }
}

/// A role payload that can be wrapped in a [`Signed`] envelope.
pub trait Role: Serialize {}

/// A single signature over a metadata file's canonical `signed` bytes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Signature {
    /// The signing key's TUF key ID.
    pub keyid: String,
    /// Hex-encoded raw Ed25519 signature.
    pub sig: String,
    /// Additional fields defined by a compatible TUF producer.
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

/// A metadata file: a role payload plus the signatures over it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Signed<T> {
    /// Signatures over the canonical bytes of `signed`, ordered by key ID.
    pub signatures: Vec<Signature>,
    /// The role payload.
    pub signed: T,
    /// Additional fields defined by a compatible TUF producer.
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

impl<T: Role> Signed<T> {
    /// Sign `payload` with every key in `keys`.
    ///
    /// Fails if two keys share a key ID: TUF requires key IDs to be unique in
    /// the signatures array, and a duplicate never adds threshold weight.
    pub fn new(payload: T, keys: &[&KeyPair]) -> Result<Self> {
        let canonical = canonical::to_bytes(&payload)?;
        let mut signatures = keys
            .iter()
            .map(|key| {
                Ok(Signature {
                    keyid: key.key_id()?,
                    sig: key.sign_hex(&canonical),
                    extra: BTreeMap::new(),
                })
            })
            .collect::<Result<Vec<_>>>()?;
        signatures.sort_by(|a, b| a.keyid.cmp(&b.keyid));
        if let Some(pair) = signatures.windows(2).find(|w| w[0].keyid == w[1].keyid) {
            return Err(Error::DuplicateKeyId(pair[0].keyid.clone()));
        }
        Ok(Self {
            signatures,
            signed: payload,
            extra: BTreeMap::new(),
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
    /// Additional fields defined by a compatible TUF producer.
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
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
    /// Optional in the spec; absent means `false`.
    #[serde(default)]
    pub consistent_snapshot: bool,
    /// Every key referenced by a role, indexed by key ID.
    pub keys: BTreeMap<String, PublicKey>,
    /// Role name to its authorized keys and threshold.
    pub roles: BTreeMap<String, RoleKeys>,
    /// Additional fields defined by a compatible TUF producer.
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

/// The keys one role is authorized with, and how many of them a metadata file
/// needs signatures from.
///
/// `keys` may hold more than `threshold` demands — that is what a 2-of-3 root
/// is, and it is the whole reason this is a list rather than a key. The keys
/// beyond the threshold are what makes a lost or compromised holder survivable
/// without re-rooting every client.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoleAuthorization {
    /// The role being authorized.
    pub role: RoleName,
    /// Every key allowed to sign for it.
    pub keys: Vec<PublicKey>,
    /// How many distinct ones a valid signature set needs.
    pub threshold: u32,
}

impl RoleAuthorization {
    /// Authorize `role` with a single key at threshold 1.
    #[must_use]
    pub fn single(role: RoleName, key: PublicKey) -> Self {
        Self {
            role,
            keys: vec![key],
            threshold: 1,
        }
    }

    /// The role's key IDs, sorted, and the keys indexed by them.
    ///
    /// Rejects a threshold no key set can meet and a key authorized twice.
    /// Both produce a role that verifies differently from how it reads — the
    /// first never verifies at all, the second silently lowers the threshold —
    /// and both are far cheaper to catch here than at a ceremony's end.
    fn resolve(&self) -> Result<(RoleKeys, BTreeMap<String, PublicKey>)> {
        if self.threshold == 0 || self.threshold as usize > self.keys.len() {
            return Err(Error::UnsatisfiableThreshold {
                role: self.role,
                threshold: self.threshold,
                keys: self.keys.len(),
            });
        }

        let mut indexed = BTreeMap::new();
        for key in &self.keys {
            let key_id = key.key_id()?;
            if indexed.insert(key_id.clone(), key.clone()).is_some() {
                return Err(Error::DuplicateRoleKey {
                    role: self.role,
                    key_id,
                });
            }
        }

        Ok((
            RoleKeys {
                keyids: indexed.keys().cloned().collect(),
                threshold: self.threshold,
                extra: BTreeMap::new(),
            },
            indexed,
        ))
    }
}

impl Root {
    /// Build a root from an explicit authorization per role.
    ///
    /// The `keys` map is shared across roles by key ID, so a key authorized for
    /// two roles is stored once — which is what the TUF object shape asks for,
    /// not a deduplication convenience.
    pub fn new(
        version: u64,
        expires: String,
        authorizations: &[RoleAuthorization],
    ) -> Result<Self> {
        let mut keys = BTreeMap::new();
        let mut roles = BTreeMap::new();
        for authorization in authorizations {
            let (role_keys, role_key_map) = authorization.resolve()?;
            roles.insert(authorization.role.as_str().to_string(), role_keys);
            keys.extend(role_key_map);
        }
        Ok(Self {
            type_: RoleName::Root.as_str().to_string(),
            spec_version: policy::SPEC_VERSION.to_string(),
            version,
            expires,
            consistent_snapshot: true,
            keys,
            roles,
            extra: BTreeMap::new(),
        })
    }

    /// Build a root that authorizes exactly one key per role at threshold 1.
    pub fn single_key_per_role(
        version: u64,
        expires: String,
        role_keys: &[(RoleName, PublicKey)],
    ) -> Result<Self> {
        let authorizations: Vec<RoleAuthorization> = role_keys
            .iter()
            .map(|(role, public)| RoleAuthorization::single(*role, public.clone()))
            .collect();
        Self::new(version, expires, &authorizations)
    }
}

impl Role for Root {}

/// A reference to another metadata file, as recorded by a parent role.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MetaFile {
    /// The referenced file's version.
    pub version: u64,
    /// Its exact length in bytes. Optional in the spec; this publisher always
    /// records it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub length: Option<u64>,
    /// Its hashes, algorithm to hex digest. Optional in the spec; this
    /// publisher always records them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hashes: Option<BTreeMap<String, String>>,
    /// Additional fields defined by a compatible TUF producer.
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

impl MetaFile {
    /// Pin `bytes` — the exact published file — at `version`.
    #[must_use]
    pub fn pinning(version: u64, bytes: &[u8]) -> Self {
        Self {
            version,
            length: Some(bytes.len() as u64),
            hashes: Some(sha256_map(bytes)),
            extra: BTreeMap::new(),
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
    /// Additional fields defined by a compatible TUF producer.
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
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
    /// Additional fields defined by a compatible TUF producer.
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
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
            extra: BTreeMap::new(),
        }
    }
}

impl Role for Targets {}

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
    /// Additional fields defined by a compatible TUF producer.
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
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
            extra: BTreeMap::new(),
        }
    }
}

impl Role for Snapshot {}

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
    /// Additional fields defined by a compatible TUF producer.
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
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
            extra: BTreeMap::new(),
        }
    }
}

impl Role for Timestamp {}

/// The `{"sha256": "<hex>"}` hash map TUF records for a file.
#[must_use]
pub fn sha256_map(bytes: &[u8]) -> BTreeMap<String, String> {
    BTreeMap::from([("sha256".to_string(), hex::encode(Sha256::digest(bytes)))])
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::de::DeserializeOwned;

    fn assert_json_round_trip<T>(json: &str)
    where
        T: DeserializeOwned + Serialize,
    {
        let expected: serde_json::Value = serde_json::from_str(json).unwrap();
        let parsed: T = serde_json::from_value(expected.clone()).unwrap();
        assert_eq!(serde_json::to_value(parsed).unwrap(), expected);
    }

    #[test]
    fn a_role_name_honours_the_width_it_is_formatted_with() {
        assert_eq!(format!("{:<10}|", RoleName::Root), "root      |");
        assert_eq!(format!("{}", RoleName::Timestamp), "timestamp");
    }

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
    fn signing_with_a_duplicate_key_is_rejected() {
        let key = KeyPair::from_seed(&[1u8; 32]);
        let targets = Targets::new(1, "2027-01-01T00:00:00Z".to_string(), BTreeMap::new());
        let err = Signed::new(targets, &[&key, &key]).unwrap_err();
        assert!(matches!(err, Error::DuplicateKeyId(id) if id == key.key_id().unwrap()));
    }

    #[test]
    fn spec_minimal_metadata_deserializes() {
        // §4.4 allows a snapshot meta entry with only a version, and §4.3
        // allows root to omit consistent_snapshot (meaning false).
        let meta: MetaFile = serde_json::from_str(r#"{"version": 1}"#).unwrap();
        assert_eq!(meta.version, 1);
        assert_eq!(meta.length, None);
        assert_eq!(meta.hashes, None);
        assert_eq!(serde_json::to_string(&meta).unwrap(), r#"{"version":1}"#);

        let root: Root = serde_json::from_str(
            r#"{
                "_type": "root",
                "spec_version": "1.0.33",
                "version": 1,
                "expires": "2027-01-01T00:00:00Z",
                "keys": {},
                "roles": {}
            }"#,
        )
        .unwrap();
        assert!(!root.consistent_snapshot);
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
    fn compatible_tuf_extensions_survive_round_trips() {
        assert_json_round_trip::<Signed<Root>>(
            r#"{
                "signatures": [{"keyid": "id", "sig": "00", "x-signature": true}],
                "signed": {
                    "_type": "root",
                    "spec_version": "1.0.33",
                    "version": 1,
                    "expires": "2027-01-01T00:00:00Z",
                    "consistent_snapshot": true,
                    "keys": {
                        "id": {
                            "keytype": "ed25519",
                            "scheme": "ed25519",
                            "keyval": {"public": "00", "x-keyval": 1},
                            "x-key": 2
                        }
                    },
                    "roles": {
                        "root": {"keyids": ["id"], "threshold": 1, "x-role": 3}
                    },
                    "x-root": 4
                },
                "x-envelope": 5
            }"#,
        );
        assert_json_round_trip::<Signed<Targets>>(
            r#"{
                "signatures": [],
                "signed": {
                    "_type": "targets",
                    "spec_version": "1.0.33",
                    "version": 1,
                    "expires": "2027-01-01T00:00:00Z",
                    "targets": {
                        "app": {"length": 1, "hashes": {}, "x-target": true}
                    },
                    "x-targets": true
                }
            }"#,
        );
        assert_json_round_trip::<Signed<Snapshot>>(
            r#"{
                "signatures": [],
                "signed": {
                    "_type": "snapshot",
                    "spec_version": "1.0.33",
                    "version": 1,
                    "expires": "2027-01-01T00:00:00Z",
                    "meta": {
                        "1.targets.json": {
                            "version": 1,
                            "length": 1,
                            "hashes": {},
                            "x-meta": true
                        }
                    },
                    "x-snapshot": true
                }
            }"#,
        );
        assert_json_round_trip::<Signed<Timestamp>>(
            r#"{
                "signatures": [],
                "signed": {
                    "_type": "timestamp",
                    "spec_version": "1.0.33",
                    "version": 1,
                    "expires": "2027-01-01T00:00:00Z",
                    "meta": {
                        "snapshot.json": {"version": 1, "length": 1, "hashes": {}}
                    },
                    "x-timestamp": true
                }
            }"#,
        );
    }

    fn seeded(byte: u8) -> KeyPair {
        KeyPair::from_seed(&[byte; 32])
    }

    fn expires() -> String {
        "2027-01-01T00:00:00Z".to_string()
    }

    /// The shape a root ceremony produces: three holders, any two of whom can
    /// sign, while the online roles stay single-key.
    #[test]
    fn root_authorizes_several_keys_at_a_threshold() {
        let holders: Vec<KeyPair> = (10..13).map(seeded).collect();
        let mut authorizations = vec![RoleAuthorization {
            role: RoleName::Root,
            keys: holders.iter().map(KeyPair::public).collect(),
            threshold: 2,
        }];
        authorizations.extend(
            [RoleName::Targets, RoleName::Snapshot, RoleName::Timestamp]
                .into_iter()
                .enumerate()
                .map(|(i, role)| {
                    RoleAuthorization::single(role, seeded(u8::try_from(i).unwrap()).public())
                }),
        );

        let root = Root::new(1, expires(), &authorizations).unwrap();

        let entry = &root.roles["root"];
        assert_eq!(entry.threshold, 2);
        assert_eq!(entry.keyids.len(), 3);
        // Six keys in total: three root holders and one per online role.
        assert_eq!(root.keys.len(), 6);
        for key_id in &entry.keyids {
            assert!(root.keys.contains_key(key_id));
        }
        assert!(entry.keyids.windows(2).all(|w| w[0] < w[1]));
    }

    /// A key authorized for two roles is one entry in `keys`, referenced from
    /// both roles.
    #[test]
    fn a_key_shared_between_roles_is_stored_once() {
        let shared = seeded(4);
        let root = Root::new(
            1,
            expires(),
            &[
                RoleAuthorization::single(RoleName::Snapshot, shared.public()),
                RoleAuthorization::single(RoleName::Timestamp, shared.public()),
            ],
        )
        .unwrap();

        assert_eq!(root.keys.len(), 1);
        assert_eq!(
            root.roles["snapshot"].keyids,
            root.roles["timestamp"].keyids
        );
    }

    #[test]
    fn a_threshold_no_key_set_could_meet_is_refused() {
        let error = Root::new(
            1,
            expires(),
            &[RoleAuthorization {
                role: RoleName::Root,
                keys: vec![seeded(1).public(), seeded(2).public()],
                threshold: 3,
            }],
        )
        .unwrap_err();

        assert!(matches!(
            error,
            Error::UnsatisfiableThreshold {
                role: RoleName::Root,
                threshold: 3,
                keys: 2
            }
        ));
    }

    #[test]
    fn a_zero_threshold_is_refused() {
        let error = Root::new(
            1,
            expires(),
            &[RoleAuthorization {
                role: RoleName::Root,
                keys: vec![seeded(1).public()],
                threshold: 0,
            }],
        )
        .unwrap_err();

        assert!(matches!(
            error,
            Error::UnsatisfiableThreshold { threshold: 0, .. }
        ));
    }

    /// Writing the same holder twice would read as a 2-of-3 and verify as a
    /// 2-of-2 — the one mistake at a ceremony that leaves no trace.
    #[test]
    fn a_key_authorized_twice_for_one_role_is_refused() {
        let holder = seeded(1);
        let error = Root::new(
            1,
            expires(),
            &[RoleAuthorization {
                role: RoleName::Root,
                keys: vec![holder.public(), seeded(2).public(), holder.public()],
                threshold: 2,
            }],
        )
        .unwrap_err();

        assert!(matches!(
            error,
            Error::DuplicateRoleKey {
                role: RoleName::Root,
                ..
            }
        ));
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
