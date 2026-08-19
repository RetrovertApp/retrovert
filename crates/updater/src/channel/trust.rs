//! Trust state: the newest verified metadata, and the version floor beneath
//! which this client refuses to go back.
//!
//! Both halves live wherever the caller keeps durable state, never in a cache.
//! A cache is evictable by definition, and a floor that can be evicted is not a
//! floor: with it gone, a host serving an older — still signed, still
//! unexpired — chain is accepted. The metadata half alone does not close that,
//! because metadata that has since expired no longer seeds a floor at all.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sigstore_tuf::{FileStore, MetadataStore, TrustedMetadataSet};

use super::error::{Error, Result};

/// Where the version floor is recorded, relative to the trust directory.
const FLOOR_FILE: &str = "floor.json";

/// Where verified metadata is kept, relative to the trust directory.
const METADATA_DIR: &str = "metadata";

/// The lowest metadata version this client accepts for each top-level role.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Floor {
    /// Floor for the `root` role.
    pub root: u64,
    /// Floor for the `timestamp` role.
    pub timestamp: u64,
    /// Floor for the `snapshot` role.
    pub snapshot: u64,
    /// Floor for the `targets` role.
    pub targets: u64,
}

impl Floor {
    /// The versions a completed refresh left trusted.
    pub fn of(trusted: &TrustedMetadataSet) -> Result<Self> {
        Ok(Self {
            root: trusted.root().version,
            timestamp: trusted
                .timestamp()
                .ok_or(Error::IncompleteRefresh("timestamp"))?
                .version,
            snapshot: trusted
                .snapshot()
                .ok_or(Error::IncompleteRefresh("snapshot"))?
                .version,
            targets: trusted
                .targets()
                .ok_or(Error::IncompleteRefresh("targets"))?
                .version,
        })
    }

    /// Refuse `offered` if any role in it sits below this floor.
    pub fn admit(&self, offered: &Self) -> Result<()> {
        for (role, floor, offered) in [
            ("root", self.root, offered.root),
            ("timestamp", self.timestamp, offered.timestamp),
            ("snapshot", self.snapshot, offered.snapshot),
            ("targets", self.targets, offered.targets),
        ] {
            if offered < floor {
                return Err(Error::Rollback {
                    role,
                    floor,
                    offered,
                });
            }
        }
        Ok(())
    }

    /// This floor lifted to whichever of the two is higher, role by role.
    ///
    /// Never lowers: a check that authenticates an unchanged chain leaves the
    /// floor where it was rather than pulling it down to match.
    #[must_use]
    pub fn raised_to(&self, offered: &Self) -> Self {
        Self {
            root: self.root.max(offered.root),
            timestamp: self.timestamp.max(offered.timestamp),
            snapshot: self.snapshot.max(offered.snapshot),
            targets: self.targets.max(offered.targets),
        }
    }
}

/// A directory holding one channel's trust state.
#[derive(Debug, Clone)]
pub struct TrustStore {
    dir: PathBuf,
}

impl TrustStore {
    /// Keep trust state under `dir`, which is created when first written to.
    ///
    /// `dir` must not be inside a download cache or any other directory a
    /// janitor is free to clear, and belongs to one channel client at a time:
    /// raising the floor is a read-modify-write, and nothing here locks across
    /// processes.
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    /// The directory the state lives in.
    #[must_use]
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// The floor recorded by the last successful check.
    ///
    /// Only an absent record starts at zero, and only because a client that has
    /// never checked has nothing to refuse yet. Every other failure to read one
    /// is raised: a record that exists and cannot be read is a floor, and
    /// treating it as absent would admit the rollback it was written to refuse
    /// — then persist the mirror's own versions in its place.
    pub fn floor(&self) -> Result<Floor> {
        let path = self.floor_path();
        let bytes = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Floor::default()),
            Err(e) => return Err(Error::trust(path, e)),
        };
        serde_json::from_slice(&bytes).map_err(|e| Error::trust(path, e))
    }

    /// Record `floor` as the new refusal boundary.
    pub(super) fn record(&self, floor: &Floor) -> Result<()> {
        let bytes = serde_json::to_vec(floor).map_err(|e| Error::trust(self.floor_path(), e))?;
        // Through the metadata store's blob writer, so the record lands
        // atomically and a crash mid-write cannot leave a truncated floor.
        self.state()
            .store(FLOOR_FILE, &bytes)
            .map_err(|e| Error::trust(self.floor_path(), e))
    }

    /// The store verified metadata is written through to.
    pub(super) fn metadata(&self) -> FileStore {
        FileStore::new(self.dir.join(METADATA_DIR))
    }

    fn state(&self) -> FileStore {
        FileStore::new(&self.dir)
    }

    fn floor_path(&self) -> PathBuf {
        self.dir.join(FLOOR_FILE)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn versions(root: u64, timestamp: u64, snapshot: u64, targets: u64) -> Floor {
        Floor {
            root,
            timestamp,
            snapshot,
            targets,
        }
    }

    #[test]
    fn a_chain_at_or_above_the_floor_is_admitted() {
        let floor = versions(1, 4, 4, 4);
        assert!(floor.admit(&floor).is_ok());
        assert!(floor.admit(&versions(2, 5, 5, 5)).is_ok());
    }

    #[test]
    fn any_role_below_the_floor_is_refused_by_name() {
        let floor = versions(2, 4, 4, 4);
        for (role, offered) in [
            ("root", versions(1, 4, 4, 4)),
            ("timestamp", versions(2, 3, 4, 4)),
            ("snapshot", versions(2, 4, 3, 4)),
            ("targets", versions(2, 4, 4, 3)),
        ] {
            let err = floor.admit(&offered).unwrap_err();
            assert!(
                matches!(err, Error::Rollback { role: named, .. } if named == role),
                "{role} regression reported as {err}"
            );
        }
    }

    #[test]
    fn a_floor_only_ever_rises() {
        let floor = versions(2, 9, 4, 4);
        assert_eq!(floor.raised_to(&versions(1, 4, 5, 4)), versions(2, 9, 5, 4));
        assert_eq!(floor.raised_to(&Floor::default()), floor);
    }

    #[test]
    fn a_floor_round_trips_through_the_store_and_starts_at_zero() {
        let dir = tempfile::tempdir().unwrap();
        let store = TrustStore::new(dir.path().join("trust"));
        assert_eq!(store.floor().unwrap(), Floor::default());

        let recorded = versions(2, 9, 5, 4);
        store.record(&recorded).unwrap();
        assert_eq!(store.floor().unwrap(), recorded);
        assert_eq!(TrustStore::new(store.dir()).floor().unwrap(), recorded);
    }

    #[test]
    fn a_malformed_floor_is_refused_rather_than_read_as_absent() {
        let dir = tempfile::tempdir().unwrap();
        let store = TrustStore::new(dir.path());
        std::fs::write(dir.path().join(FLOOR_FILE), b"{ not json").unwrap();

        let err = store.floor().unwrap_err();
        assert!(matches!(err, Error::Trust { .. }), "{err}");
    }

    #[test]
    fn a_floor_that_cannot_be_read_at_all_is_refused_too() {
        let dir = tempfile::tempdir().unwrap();
        let store = TrustStore::new(dir.path());
        // A directory where the record belongs: present, and unreadable as
        // bytes. Any other read failure — a permission or a device error —
        // arrives here the same way, and none of them may read as "no floor".
        std::fs::create_dir(dir.path().join(FLOOR_FILE)).unwrap();

        let err = store.floor().unwrap_err();
        assert!(matches!(err, Error::Trust { .. }), "{err}");
    }

    #[test]
    fn a_partial_record_keeps_the_roles_it_names() {
        let dir = tempfile::tempdir().unwrap();
        let store = TrustStore::new(dir.path());
        std::fs::write(dir.path().join(FLOOR_FILE), br#"{"timestamp":7}"#).unwrap();

        assert_eq!(store.floor().unwrap(), versions(0, 7, 0, 0));
    }
}
