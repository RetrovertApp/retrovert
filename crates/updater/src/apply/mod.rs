//! Turning a plan into a verified installed generation, and the point where
//! this crate's job ends.
//!
//! The second half of the two-phase update. Artifacts are fetched through the
//! transfer queue at the priority the caller asks for, so the queue's own
//! validation is what every acquired byte passes through; the copy that lands
//! in the generation proves the same digest again before it is published,
//! through the same check rather than a second implementation of it.
//!
//! Publication is committed in the order the Replay database updater proved:
//! every file lands first, and only once all of them have does the generation
//! get recorded. Nothing ever advertises a half-applied generation, and an
//! apply interrupted anywhere leaves the generation already installed
//! resolvable.
//!
//! Failure is not fatal. A hash or rollback refusal quarantines the download
//! it came from and returns an error; the installed generation is not touched,
//! and a player whose update fails keeps playing.

mod error;
mod install;

#[cfg(test)]
mod generations;

use std::collections::{HashMap, HashSet, VecDeque};
use std::fs::{self, File};
use std::io;
use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::Duration;

use retrovert_tuf::manifest::Artifact;

pub use error::{Error, Result};
pub use install::{Generation, InstallRoot, Installed};

use crate::check::Plan;
use crate::queue::{EntryId, Failure, Priority, Request, State, TransferQueue, verify_on_disk};
use crate::transport::ArtifactDigest;

/// How often an apply looks at the transfers it queued. The queue publishes
/// terminal state through atomics and offers nothing to wait on, so this is a
/// poll rather than a park.
const POLL: Duration = Duration::from_millis(20);

/// Publishes plans into the install root, and reports each generation once.
pub struct Applier {
    queue: Arc<TransferQueue>,
    install: InstallRoot,
    base: String,
    completions: Completions,
}

impl std::fmt::Debug for Applier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Applier")
            .field("install", &self.install.root())
            .field("base", &self.base)
            .finish_non_exhaustive()
    }
}

impl Applier {
    /// Publish beneath `install`, fetching artifacts through `queue` from
    /// `artifact_base_url`.
    ///
    /// An artifact's manifest path is resolved against that base, which is the
    /// release's own — the channel's metadata namespace is a separate one and
    /// is never fetched from here.
    #[must_use]
    pub fn new(queue: Arc<TransferQueue>, install: InstallRoot, artifact_base_url: &str) -> Self {
        Self {
            queue,
            install,
            base: format!("{}/", artifact_base_url.trim_end_matches('/')),
            completions: Completions::default(),
        }
    }

    /// The install root generations are published into.
    ///
    /// Clone it to run retention from another thread: a clone shares the
    /// publish claims, so it cannot collect a generation this applier is still
    /// writing.
    #[must_use]
    pub fn install_root(&self) -> &InstallRoot {
        &self.install
    }

    /// The generations this applier has published, each offered once.
    #[must_use]
    pub fn completions(&self) -> Completions {
        self.completions.clone()
    }

    /// Acquire everything `plan` names and publish it as a generation.
    ///
    /// A plan whose generation is already installed returns it without
    /// fetching anything: a generation is named by the digest of the manifest
    /// that produced it, so an identical release set is the generation on disk
    /// rather than another copy of it.
    ///
    /// Blocks until every artifact has landed or one of them has failed. A
    /// failure leaves the installed generation untouched and quarantines the
    /// download that caused it.
    pub fn apply(&self, plan: &Plan, priority: Priority) -> Result<Generation> {
        // The id names a directory and the paths name files inside it. The
        // sanctioned flow produces both from a parsed manifest, but a plan is
        // plain data a consumer could have stored or rebuilt, so what reaches
        // the filesystem is re-proven here rather than trusted.
        refuse_unsafe_names(plan)?;
        let generation = match self.install.resolve(&plan.generation_id) {
            Some(installed) => installed,
            None => self.publish(plan, priority)?,
        };
        self.completions.record(&generation);
        Ok(generation)
    }

    fn publish(&self, plan: &Plan, priority: Priority) -> Result<Generation> {
        let _claim = self.install.claim(&plan.generation_id);
        let dir = self.install.dir_of(&plan.generation_id);
        refuse_collisions(&plan.artifacts)?;
        fs::create_dir_all(&dir).map_err(|e| Error::install(&dir, e))?;

        let installed = self.acquire(plan, priority, &dir)?;
        self.install.record(&plan.generation_id, installed)
    }

    /// Fetch every artifact and place it, returning them in manifest order.
    ///
    /// Transfers run as concurrently as the queue allows: everything the queue
    /// has room for is in flight at once, and an artifact is placed as soon as
    /// it lands rather than after the slowest one has.
    fn acquire(&self, plan: &Plan, priority: Priority, dir: &Path) -> Result<Vec<Installed>> {
        let mut placed: Vec<Option<Installed>> = vec![None; plan.artifacts.len()];
        let mut queued = Acquisition::new(&self.queue);
        let mut next = 0;

        while next < plan.artifacts.len() || !queued.active.is_empty() {
            let mut progressed = false;

            while next < plan.artifacts.len() {
                let artifact = &plan.artifacts[next];
                let Some(id) = self.queue.queue(self.request_for(artifact, priority)?) else {
                    // Every slot is in use; the sweep below frees some.
                    break;
                };
                queued.active.push((id, next));
                next += 1;
                progressed = true;
            }

            // Swept in place: an entry leaves the tracked list only once its
            // slot is free, so nothing is in the acquisition's blind spot if a
            // settle unwinds, and a failure partway leaves the rest tracked
            // for the drop to release.
            let mut cursor = 0;
            let mut refused = None;
            while cursor < queued.active.len() {
                let (id, index) = queued.active[cursor];
                match self.settle(dir, &plan.artifacts[index], id) {
                    Ok(Some(installed)) => {
                        progressed = true;
                        placed[index] = Some(installed);
                        let _ = self.queue.remove(id);
                        queued.active.swap_remove(cursor);
                    }
                    Ok(None) => cursor += 1,
                    Err(e) => {
                        refused = Some(e);
                        break;
                    }
                }
            }
            if let Some(refused) = refused {
                return Err(refused);
            }

            if !progressed {
                thread::sleep(POLL);
            }
        }

        Ok(placed.into_iter().flatten().collect())
    }

    /// Place `artifact` if its transfer has landed, or report that it has not
    /// yet reached a terminal state.
    ///
    /// Leaves the entry in its slot either way: freeing it is the caller's, so
    /// that every entry is released exactly once and a failure partway through
    /// still releases the rest.
    fn settle(&self, dir: &Path, artifact: &Artifact, id: EntryId) -> Result<Option<Installed>> {
        match self.queue.state(id) {
            State::Complete => {
                let cached = self.queue.path(id);
                self.place(dir, artifact, cached.as_deref()).map(Some)
            }
            State::Failed => Err(Error::Artifact {
                name: artifact.name.clone(),
                source: self.queue.failure(id).unwrap_or(Failure::Panicked),
            }),
            State::Cancelled => Err(Error::Cancelled {
                name: artifact.name.clone(),
            }),
            State::Pending | State::Downloading | State::Paused | State::Preempted => Ok(None),
        }
    }

    fn request_for(&self, artifact: &Artifact, priority: Priority) -> Result<Request> {
        Ok(Request {
            url: format!("{}{}", self.base, artifact.path),
            digest: digest_of(artifact)?,
            expected_size: Some(artifact.size),
            priority,
        })
    }

    /// Copy one acquired artifact into the generation, verified.
    ///
    /// It lands under a temporary name, proves the digest the manifest named
    /// while still under it, and is renamed into place only then — so the
    /// generation directory never holds a file that has not been verified.
    fn place(&self, dir: &Path, artifact: &Artifact, cached: Option<&Path>) -> Result<Installed> {
        let digest = digest_of(artifact)?;
        let dest = dir.join(&artifact.path);
        let parent = dest.parent().unwrap_or(dir);
        fs::create_dir_all(parent).map_err(|e| Error::install(parent, e))?;

        let quarantine = |error| {
            // The bytes that produced this copy cannot be trusted, and the
            // cache entry is what the next attempt would take a hit on.
            self.queue.transport().cache().evict(&digest);
            error
        };

        let source = cached.ok_or_else(|| {
            quarantine(Error::Artifact {
                name: artifact.name.clone(),
                source: Failure::Unreadable,
            })
        })?;

        let landed = copy_durably(source, parent).map_err(|e| Error::install(&dest, e))?;
        if let Err(refused) = verify_on_disk(landed.path(), &digest) {
            return Err(quarantine(Error::Artifact {
                name: artifact.name.clone(),
                source: refused,
            }));
        }
        landed
            .persist(&dest)
            .map_err(|e| Error::install(&dest, e.error))?;

        Ok(Installed {
            name: artifact.name.clone(),
            target: artifact.target.clone(),
            version: artifact.version.clone(),
            revision: artifact.revision.clone(),
            sha256: digest.to_string(),
            size: artifact.size,
            path: artifact.path.clone(),
        })
    }
}

/// The transfers one apply still holds, released whichever way the apply ends.
///
/// An apply that returns early — or unwinds — must not leave slots claimed:
/// the queue is shared, and a leaked slot is capacity nobody can reclaim.
struct Acquisition<'a> {
    queue: &'a TransferQueue,
    active: Vec<(EntryId, usize)>,
}

impl<'a> Acquisition<'a> {
    fn new(queue: &'a TransferQueue) -> Self {
        Self {
            queue,
            active: Vec::new(),
        }
    }
}

impl Drop for Acquisition<'_> {
    /// Cancel what is still in flight and wait it out before freeing the
    /// slots, since the queue refuses to free an active one.
    ///
    /// A cancel is observed between reads, so this waits out the read in
    /// flight — the same bound [`TransferQueue`]'s own drop accepts.
    fn drop(&mut self) {
        for (id, _) in &self.active {
            let _ = self.queue.cancel(*id);
        }
        for (id, _) in &self.active {
            while !self.queue.remove(*id) {
                thread::sleep(POLL);
            }
        }
    }
}

/// The generations an applier has published, each offered to a reader once.
///
/// Cloning shares the record, so the thread that applies and the thread that
/// activates can hold their own handles. The record is this process's: a
/// restart offers the generation it then applies again, which is what a
/// consumer that has just started needs to hear.
#[derive(Debug, Clone, Default)]
pub struct Completions {
    inner: Arc<Mutex<Reported>>,
}

#[derive(Debug, Default)]
struct Reported {
    seen: HashSet<String>,
    pending: VecDeque<Generation>,
}

impl Completions {
    /// The next generation this handle has not been told about, if any.
    #[must_use]
    pub fn next_completed(&self) -> Option<Generation> {
        self.lock().pending.pop_front()
    }

    fn record(&self, generation: &Generation) {
        let mut reported = self.lock();
        if reported.seen.insert(generation.id().to_string()) {
            reported.pending.push_back(generation.clone());
        }
    }

    fn lock(&self) -> MutexGuard<'_, Reported> {
        // A prior panic left the record usable, so recover rather than
        // propagate.
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

fn digest_of(artifact: &Artifact) -> Result<ArtifactDigest> {
    ArtifactDigest::from_hex(&artifact.sha256).map_err(|_| Error::Digest {
        name: artifact.name.clone(),
        digest: artifact.sha256.clone(),
    })
}

/// Refuse a plan whose names cannot safely reach the filesystem: a generation
/// id that is not a manifest digest, or an artifact path that could climb out
/// of the generation directory.
///
/// A plan built from an authenticated manifest always passes — the manifest's
/// own validation is stricter — so this only ever stops a plan that did not
/// come from one.
fn refuse_unsafe_names(plan: &Plan) -> Result<()> {
    if !retrovert_tuf::manifest::is_hex_sha256(&plan.generation_id) {
        return Err(Error::GenerationId {
            id: plan.generation_id.clone(),
        });
    }
    for artifact in &plan.artifacts {
        if !retrovert_tuf::manifest::is_clean_relative_path(&artifact.path) {
            return Err(Error::UnsafePath {
                name: artifact.name.clone(),
                path: artifact.path.clone(),
            });
        }
    }
    Ok(())
}

/// Refuse a plan whose artifacts want the same place in the generation.
///
/// A manifest keeps name-and-target pairs unique but says nothing about paths,
/// and two artifacts that both apply to this consumer could name one. Placing
/// them in sequence would publish whichever landed last under both names.
fn refuse_collisions(artifacts: &[Artifact]) -> Result<()> {
    let mut claimed: HashMap<&str, &str> = HashMap::new();
    for artifact in artifacts {
        if let Some(first) = claimed.insert(&artifact.path, &artifact.name) {
            return Err(Error::Collision {
                first: first.to_string(),
                second: artifact.name.clone(),
                path: artifact.path.clone(),
            });
        }
    }
    Ok(())
}

/// Copy `source` into a temporary file in `dir` and flush its bytes.
///
/// The flush stops the record reaching the disk ahead of the contents it
/// describes. The directory entries are not fsynced — the same bound
/// `sigstore-tuf`'s atomic write works to, and what the rest of this crate's
/// durable state already assumes.
fn copy_durably(source: &Path, dir: &Path) -> io::Result<tempfile::NamedTempFile> {
    let mut from = File::open(source)?;
    let mut landing = tempfile::NamedTempFile::new_in(dir)?;
    io::copy(&mut from, landing.as_file_mut())?;
    landing.as_file().sync_all()?;
    Ok(landing)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn artifact(name: &str, path: &str) -> Artifact {
        Artifact {
            name: name.to_string(),
            target: None,
            path: path.to_string(),
            sha256: "ab".repeat(32),
            size: 1,
            revision: "abc1234".to_string(),
            version: None,
        }
    }

    fn plan_of(id: &str, artifacts: Vec<Artifact>) -> Plan {
        Plan {
            generation_id: id.to_string(),
            artifacts,
            total_bytes: 0,
            cached_bytes: 0,
        }
    }

    #[test]
    fn a_generation_id_that_is_not_a_manifest_digest_is_refused() {
        for bad in ["", "aa", "../escape", &"A".repeat(64)] {
            let err = refuse_unsafe_names(&plan_of(bad, Vec::new())).unwrap_err();
            assert!(
                matches!(&err, Error::GenerationId { id } if id == bad),
                "{err}"
            );
        }
        let digest_shaped = "ab".repeat(32);
        assert!(
            refuse_unsafe_names(&plan_of(
                &digest_shaped,
                vec![artifact("spu", "spu.tar.zst")]
            ))
            .is_ok()
        );
    }

    #[test]
    fn an_artifact_path_that_could_escape_the_generation_is_refused() {
        let id = "ab".repeat(32);
        for bad in [
            "../outside",
            "/etc/passwd",
            "c:/evil",
            "app?x=1",
            "dir\\app",
        ] {
            let err = refuse_unsafe_names(&plan_of(&id, vec![artifact("spu", bad)])).unwrap_err();
            assert!(
                matches!(&err, Error::UnsafePath { path, .. } if path == bad),
                "{err}"
            );
        }
    }

    #[test]
    fn artifacts_landing_in_different_places_are_accepted() {
        let artifacts = [
            artifact("spu", "spu.tar.zst"),
            artifact("uade", "uade.tar.zst"),
        ];
        assert!(refuse_collisions(&artifacts).is_ok());
        assert!(refuse_collisions(&[]).is_ok());
    }

    #[test]
    fn two_artifacts_wanting_one_path_are_refused_by_name() {
        let artifacts = [
            artifact("spu", "plugin.tar.zst"),
            artifact("uade", "plugin.tar.zst"),
        ];
        let err = refuse_collisions(&artifacts).unwrap_err();
        assert!(
            matches!(
                &err,
                Error::Collision { first, second, path }
                    if first == "spu" && second == "uade" && path == "plugin.tar.zst"
            ),
            "{err}"
        );
    }

    #[test]
    fn a_digest_a_manifest_could_not_have_published_is_refused_by_name() {
        let mut bad = artifact("spu", "spu.tar.zst");
        bad.sha256 = "not a digest".to_string();

        let err = digest_of(&bad).unwrap_err();
        assert!(
            matches!(&err, Error::Digest { name, .. } if name == "spu"),
            "{err}"
        );
    }

    #[test]
    fn a_generation_is_offered_once_however_often_it_is_recorded() {
        let dir = tempfile::tempdir().unwrap();
        let root = InstallRoot::new(dir.path());
        let first = root.record("aa", Vec::new()).unwrap();
        let second = root.record("bb", Vec::new()).unwrap();

        let completions = Completions::default();
        completions.record(&first);
        completions.record(&first);
        completions.record(&second);

        assert_eq!(completions.next_completed().as_ref(), Some(&first));
        assert_eq!(completions.next_completed().as_ref(), Some(&second));
        assert_eq!(completions.next_completed(), None);

        // And still not again, once the reader has drained it.
        completions.record(&first);
        assert_eq!(completions.next_completed(), None);
    }

    #[test]
    fn clones_of_one_handle_share_the_record() {
        let dir = tempfile::tempdir().unwrap();
        let root = InstallRoot::new(dir.path());
        let generation = root.record("aa", Vec::new()).unwrap();

        let completions = Completions::default();
        let reader = completions.clone();
        completions.record(&generation);

        assert_eq!(reader.next_completed().as_ref(), Some(&generation));
        assert_eq!(completions.next_completed(), None);
    }

    #[test]
    fn a_copy_lands_beside_its_destination_with_the_bytes_it_was_given() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("cached");
        fs::write(&source, b"abc").unwrap();

        let landing = dir.path().join("generation");
        fs::create_dir(&landing).unwrap();
        let copied = copy_durably(&source, &landing).unwrap();

        assert_eq!(copied.path().parent(), Some(landing.as_path()));
        assert_eq!(fs::read(copied.path()).unwrap(), b"abc");
    }
}
