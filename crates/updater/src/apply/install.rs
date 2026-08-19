//! The install root: immutable generations, published by commit ordering.
//!
//! A generation is `<root>/<generation id>/`, and its record is the sibling
//! file `<root>/<generation id>.generation.json`. Every artifact lands in the
//! directory first; only once all of them have is the record written, and only
//! a generation with a record resolves. An apply interrupted anywhere before
//! that last write leaves a directory nothing resolves to and every other
//! generation untouched.
//!
//! Generations are named by the digest of the manifest that produced them, so
//! two applies of one release set are two applies of the same directory rather
//! than two directories.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use serde::{Deserialize, Serialize};
use sigstore_tuf::{FileStore, MetadataStore};

use super::error::{Error, Result};

/// The record schema this build reads and writes.
const RECORD_SCHEMA: u32 = 1;

/// The suffix a generation's record carries beside its directory.
const RECORD_SUFFIX: &str = ".generation.json";

/// One artifact as it sits in a published generation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Installed {
    /// The manifest's opaque artifact name.
    pub name: String,
    /// The manifest's opaque target selector, when it named one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    /// Human-readable version for display, when the manifest carried one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// The source commit this artifact was built from.
    pub revision: String,
    /// Hex SHA-256 of the published file, which is also what it was fetched
    /// under.
    pub sha256: String,
    /// The published file's length in bytes.
    pub size: u64,
    /// Where the file sits, `/`-separated and relative to the generation
    /// directory.
    pub path: String,
}

/// What a generation's record holds on disk.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Record {
    schema: u32,
    generation: String,
    artifacts: Vec<Installed>,
}

/// A generation that has been published in full.
///
/// Handing one back is this crate's last act. Loading the artifacts, swapping
/// a catalog over to them, and deciding when the previous generation stops
/// being live are all the consumer's.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Generation {
    id: String,
    dir: PathBuf,
    artifacts: Vec<Installed>,
}

impl Generation {
    /// The generation's id: the digest of the manifest that produced it.
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    /// The directory holding the generation's files.
    #[must_use]
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// What the generation installed, in manifest order.
    #[must_use]
    pub fn artifacts(&self) -> &[Installed] {
        &self.artifacts
    }

    /// Where `artifact` sits on disk.
    #[must_use]
    pub fn path_of(&self, artifact: &Installed) -> PathBuf {
        self.dir.join(&artifact.path)
    }
}

/// A directory of installed generations.
///
/// Cloning shares the set of generations currently being published, so a
/// janitor calling [`InstallRoot::retain`] on a clone cannot collect a
/// generation out from under an apply running on the original.
#[derive(Debug, Clone)]
pub struct InstallRoot {
    root: PathBuf,
    /// Claims per generation rather than a flat set: two applies of one
    /// release set are legitimate, and the first to finish must not drop a
    /// claim the second is still standing on.
    publishing: Arc<Mutex<HashMap<String, usize>>>,
}

impl InstallRoot {
    /// Install generations beneath `root`, which is created when the first
    /// generation is published.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            publishing: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// The directory the generations live in.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The generation `id`, if it has been published in full.
    ///
    /// A directory whose record is missing or unreadable does not resolve: it
    /// is an apply that was interrupted, and half a generation is not one.
    #[must_use]
    pub fn resolve(&self, id: &str) -> Option<Generation> {
        let bytes = fs::read(self.record_path(id)).ok()?;
        let record: Record = serde_json::from_slice(&bytes).ok()?;
        if record.schema != RECORD_SCHEMA || record.generation != id {
            return None;
        }
        Some(Generation {
            id: record.generation,
            dir: self.dir_of(id),
            artifacts: record.artifacts,
        })
    }

    /// Every generation the root has published in full, in no particular
    /// order.
    pub fn generations(&self) -> Result<Vec<Generation>> {
        Ok(self
            .ids()?
            .into_iter()
            .filter_map(|id| self.resolve(&id))
            .collect())
    }

    /// Remove every generation but the ones `live` names.
    ///
    /// A generation being published right now is kept whether or not `live`
    /// names it — the caller cannot list a generation that does not exist yet,
    /// and collecting one mid-publish would delete files an apply is still
    /// writing.
    ///
    /// Returns the ids that were removed.
    pub fn retain(&self, live: &[String]) -> Result<Vec<String>> {
        let live: HashSet<&str> = live.iter().map(String::as_str).collect();
        let mut removed = Vec::new();
        for id in self.ids()? {
            if live.contains(id.as_str()) || self.publishing().contains_key(&id) {
                continue;
            }
            // The record goes first, so a collection interrupted partway
            // through leaves a generation that no longer resolves rather than
            // one whose record outlives its files.
            let record = self.record_path(&id);
            remove_file(&record)?;
            let dir = self.dir_of(&id);
            match fs::remove_dir_all(&dir) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(Error::install(dir, e)),
            }
            removed.push(id);
        }
        Ok(removed)
    }

    /// Take the publish claim on `id` for as long as the guard lives.
    pub(super) fn claim(&self, id: &str) -> Publishing<'_> {
        *self.publishing().entry(id.to_string()).or_insert(0) += 1;
        Publishing {
            root: self,
            id: id.to_string(),
        }
    }

    /// Record `artifacts` as generation `id`, which is what makes it resolve.
    ///
    /// Called once every file has landed, and last.
    pub(super) fn record(&self, id: &str, artifacts: Vec<Installed>) -> Result<Generation> {
        let record = Record {
            schema: RECORD_SCHEMA,
            generation: id.to_string(),
            artifacts,
        };
        let bytes =
            serde_json::to_vec(&record).map_err(|e| Error::install(self.record_path(id), e))?;
        // Through the blob writer the trust state and the check log use, so a
        // crash mid-write cannot leave a truncated record behind — which would
        // otherwise advertise a generation whose contents nothing described.
        FileStore::new(&self.root)
            .store(&record_name(id), &bytes)
            .map_err(|e| Error::install(self.record_path(id), e))?;
        Ok(Generation {
            id: record.generation,
            dir: self.dir_of(id),
            artifacts: record.artifacts,
        })
    }

    pub(super) fn dir_of(&self, id: &str) -> PathBuf {
        self.root.join(id)
    }

    fn record_path(&self, id: &str) -> PathBuf {
        self.root.join(record_name(id))
    }

    fn publishing(&self) -> MutexGuard<'_, HashMap<String, usize>> {
        // A prior panic left the claims usable, so recover rather than
        // propagate.
        self.publishing
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// Every generation id the root holds, published or not.
    fn ids(&self) -> Result<Vec<String>> {
        let entries = match fs::read_dir(&self.root) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(Error::install(&self.root, e)),
        };

        let mut ids = HashSet::new();
        for entry in entries {
            let entry = entry.map_err(|e| Error::install(&self.root, e))?;
            let Ok(name) = entry.file_name().into_string() else {
                continue;
            };
            if let Some(id) = name.strip_suffix(RECORD_SUFFIX) {
                ids.insert(id.to_string());
            } else if entry.path().is_dir() {
                ids.insert(name);
            }
        }
        Ok(ids.into_iter().collect())
    }
}

/// The publish claim on one generation, released when the apply that took it
/// returns — through its error paths as much as its successful one.
pub(super) struct Publishing<'a> {
    root: &'a InstallRoot,
    id: String,
}

impl Drop for Publishing<'_> {
    fn drop(&mut self) {
        let mut publishing = self.root.publishing();
        if let Some(claims) = publishing.get_mut(&self.id) {
            *claims -= 1;
            if *claims == 0 {
                publishing.remove(&self.id);
            }
        }
    }
}

fn record_name(id: &str) -> String {
    format!("{id}{RECORD_SUFFIX}")
}

fn remove_file(path: &Path) -> Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(Error::install(path, e)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GEN_A: &str = "aa";
    const GEN_B: &str = "bb";

    fn installed(name: &str) -> Installed {
        Installed {
            name: name.to_string(),
            target: Some("linux-x86_64".to_string()),
            version: None,
            revision: "abc1234".to_string(),
            sha256: "ab".repeat(32),
            size: 3,
            path: format!("{name}.tar.zst"),
        }
    }

    /// Publish `id` the way an apply does: files first, record last.
    fn publish(root: &InstallRoot, id: &str, names: &[&str]) -> Generation {
        let dir = root.dir_of(id);
        fs::create_dir_all(&dir).unwrap();
        let artifacts: Vec<Installed> = names.iter().copied().map(installed).collect();
        for artifact in &artifacts {
            fs::write(dir.join(&artifact.path), b"abc").unwrap();
        }
        root.record(id, artifacts).unwrap()
    }

    #[test]
    fn a_recorded_generation_resolves_to_what_it_installed() {
        let dir = tempfile::tempdir().unwrap();
        let root = InstallRoot::new(dir.path().join("install"));
        let published = publish(&root, GEN_A, &["spu", "uade"]);

        let resolved = root.resolve(GEN_A).unwrap();
        assert_eq!(resolved, published);
        assert_eq!(resolved.id(), GEN_A);
        assert_eq!(resolved.dir(), root.dir_of(GEN_A));
        assert_eq!(resolved.artifacts().len(), 2);
        assert_eq!(
            resolved.path_of(&resolved.artifacts()[0]),
            root.dir_of(GEN_A).join("spu.tar.zst")
        );
        assert!(resolved.path_of(&resolved.artifacts()[0]).exists());
    }

    #[test]
    fn a_generation_whose_files_landed_but_whose_record_did_not_does_not_resolve() {
        let dir = tempfile::tempdir().unwrap();
        let root = InstallRoot::new(dir.path().join("install"));
        publish(&root, GEN_A, &["spu"]);

        let interrupted = root.dir_of(GEN_B);
        fs::create_dir_all(&interrupted).unwrap();
        fs::write(interrupted.join("uade.tar.zst"), b"abc").unwrap();

        assert!(root.resolve(GEN_B).is_none());
        // The generation already installed is untouched by the interruption.
        assert!(root.resolve(GEN_A).is_some());
        let ids: Vec<String> = root
            .generations()
            .unwrap()
            .into_iter()
            .map(|generation| generation.id)
            .collect();
        assert_eq!(ids, [GEN_A.to_string()]);
    }

    #[test]
    fn a_record_that_does_not_read_back_does_not_resolve() {
        let dir = tempfile::tempdir().unwrap();
        let root = InstallRoot::new(dir.path().join("install"));
        publish(&root, GEN_A, &["spu"]);

        fs::write(root.root().join(record_name(GEN_A)), b"{ not json").unwrap();
        assert!(root.resolve(GEN_A).is_none());
        assert!(root.generations().unwrap().is_empty());
    }

    #[test]
    fn a_record_naming_another_generation_does_not_resolve() {
        let dir = tempfile::tempdir().unwrap();
        let root = InstallRoot::new(dir.path().join("install"));
        publish(&root, GEN_A, &["spu"]);

        let stolen = fs::read(root.root().join(record_name(GEN_A))).unwrap();
        fs::write(root.root().join(record_name(GEN_B)), stolen).unwrap();
        assert!(root.resolve(GEN_B).is_none());
    }

    #[test]
    fn a_root_that_has_never_published_holds_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let root = InstallRoot::new(dir.path().join("absent"));

        assert!(root.resolve(GEN_A).is_none());
        assert!(root.generations().unwrap().is_empty());
        assert!(root.retain(&[]).unwrap().is_empty());
    }

    #[test]
    fn retention_removes_both_halves_of_everything_unlisted() {
        let dir = tempfile::tempdir().unwrap();
        let root = InstallRoot::new(dir.path().join("install"));
        publish(&root, GEN_A, &["spu"]);
        publish(&root, GEN_B, &["uade"]);

        assert_eq!(root.retain(&[GEN_A.to_string()]).unwrap(), [GEN_B]);
        assert!(root.resolve(GEN_A).is_some());
        assert!(root.resolve(GEN_B).is_none());
        assert!(!root.dir_of(GEN_B).exists());
        assert!(!root.root().join(record_name(GEN_B)).exists());
    }

    #[test]
    fn retention_collects_an_interrupted_apply_no_one_is_running() {
        let dir = tempfile::tempdir().unwrap();
        let root = InstallRoot::new(dir.path().join("install"));
        let abandoned = root.dir_of(GEN_B);
        fs::create_dir_all(&abandoned).unwrap();
        fs::write(abandoned.join("uade.tar.zst"), b"abc").unwrap();

        assert_eq!(root.retain(&[]).unwrap(), [GEN_B]);
        assert!(!abandoned.exists());
    }

    #[test]
    fn retention_never_collects_a_generation_mid_publish() {
        let dir = tempfile::tempdir().unwrap();
        let root = InstallRoot::new(dir.path().join("install"));
        let landing = root.dir_of(GEN_B);
        fs::create_dir_all(&landing).unwrap();
        fs::write(landing.join("uade.tar.zst"), b"abc").unwrap();

        // A janitor sharing the root sees the claim the apply took.
        let janitor = root.clone();
        {
            let _claim = root.claim(GEN_B);
            assert!(janitor.retain(&[]).unwrap().is_empty());
            assert!(landing.exists());
        }
        assert_eq!(janitor.retain(&[]).unwrap(), [GEN_B]);
    }

    #[test]
    fn a_generation_two_applies_are_publishing_is_held_until_both_let_go() {
        let dir = tempfile::tempdir().unwrap();
        let root = InstallRoot::new(dir.path().join("install"));
        fs::create_dir_all(root.dir_of(GEN_A)).unwrap();

        let first = root.claim(GEN_A);
        let second = root.claim(GEN_A);
        drop(first);
        assert!(root.retain(&[]).unwrap().is_empty());
        drop(second);
        assert_eq!(root.retain(&[]).unwrap(), [GEN_A]);
    }

    #[test]
    fn a_claim_is_released_even_when_the_apply_that_took_it_unwinds() {
        let dir = tempfile::tempdir().unwrap();
        let root = InstallRoot::new(dir.path().join("install"));
        let unwound = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _claim = root.claim(GEN_A);
            panic!("mid-publish");
        }));

        assert!(unwound.is_err());
        assert!(root.publishing().is_empty());
    }
}
