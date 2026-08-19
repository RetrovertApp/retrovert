//! Applying a plan into a published generation, against a real HTTP server.
//!
//! The server is in-process and hermetic (see [`fixture_server`]), so these run
//! wherever `cargo test` runs.

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

use crate::apply::{Applier, Error, InstallRoot};
use crate::check::Plan;
use crate::queue::{Config, Failure, Priority, QUEUE_MAX, Request, TransferQueue};
use crate::testing::fixture_server::{Body, FixtureServer};
use crate::transport::{ArtifactDigest, Transport};
use retrovert_tuf::manifest::Artifact;
use sha2::{Digest, Sha256};

const SPU: &str = "spu.tar.zst";
const UADE: &str = "uade.tar.zst";
const NESTED: &str = "data/db.bin";
/// A route serving bytes no manifest here ever names.
const IMPOSTOR: &str = "impostor.tar.zst";

/// A generation id the way a channel produces one: the digest of manifest
/// bytes. Nothing here parses it, so any distinct digest-shaped string does.
fn generation_id(seed: &str) -> String {
    hex::encode(Sha256::digest(seed.as_bytes()))
}

fn body_bytes(seed: u8) -> Vec<u8> {
    (0..4096u16)
        .map(|i| i.to_le_bytes()[0].wrapping_add(seed))
        .collect()
}

fn digest_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn digest(bytes: &[u8]) -> ArtifactDigest {
    ArtifactDigest::from_hex(&digest_hex(bytes)).expect("a 64-character digest")
}

fn artifact(name: &str, path: &str, bytes: &[u8]) -> Artifact {
    Artifact {
        name: name.to_string(),
        target: Some("linux-x86_64".to_string()),
        path: path.to_string(),
        sha256: digest_hex(bytes),
        size: bytes.len() as u64,
        revision: "abc1234".to_string(),
        version: Some("1.2.3".to_string()),
    }
}

fn plan(id: &str, artifacts: Vec<Artifact>) -> Plan {
    let total_bytes = artifacts.iter().map(|a| a.size).sum();
    Plan {
        generation_id: id.to_string(),
        artifacts,
        total_bytes,
        cached_bytes: 0,
    }
}

struct Fixture {
    // Declaration order is drop order: the queue blocks on its workers, so it
    // must go before the server they are talking to and the directory they are
    // writing into.
    queue: Arc<TransferQueue>,
    server: FixtureServer,
    dir: tempfile::TempDir,
    spu: Vec<u8>,
    uade: Vec<u8>,
    nested: Vec<u8>,
    /// What the impostor route serves, and what it claims to serve. Neither is
    /// any other route's bytes, so no cache hit can rescue it.
    impostor: Vec<u8>,
    claimed: Vec<u8>,
}

impl Fixture {
    fn new() -> Self {
        Self::with_workers(2)
    }

    fn with_workers(workers: usize) -> Self {
        let spu = body_bytes(1);
        let uade = body_bytes(2);
        let nested = body_bytes(3);
        let impostor = body_bytes(4);
        let claimed = body_bytes(5);

        let mut routes = HashMap::new();
        for (path, bytes, etag) in [
            (SPU, &spu, "\"spu\""),
            (UADE, &uade, "\"uade\""),
            (NESTED, &nested, "\"nested\""),
            (IMPOSTOR, &impostor, "\"impostor\""),
        ] {
            routes.insert(format!("/{path}"), Body::instant(bytes.clone(), etag));
        }

        let dir = tempfile::tempdir().expect("a scratch directory");
        let queue = TransferQueue::with_config(
            Transport::new(dir.path().join("cache")),
            Config {
                workers,
                ..Config::default()
            },
        );
        Self {
            queue: Arc::new(queue),
            server: FixtureServer::start(routes),
            dir,
            spu,
            uade,
            nested,
            impostor,
            claimed,
        }
    }

    /// An artifact whose route serves bytes that are not the ones it names.
    fn impostor(&self) -> Artifact {
        let mut lying = artifact("impostor", IMPOSTOR, &self.impostor);
        lying.sha256 = digest_hex(&self.claimed);
        lying
    }

    /// How many bodies the server has been asked for. A transfer also asks for
    /// the length first, and that is not a download.
    fn downloads(&self) -> usize {
        self.server
            .records()
            .iter()
            .filter(|record| record.method == "GET")
            .count()
    }

    fn install_root(&self) -> InstallRoot {
        InstallRoot::new(self.dir.path().join("install"))
    }

    fn applier(&self, install: InstallRoot) -> Applier {
        Applier::new(Arc::clone(&self.queue), install, &self.server.url(""))
    }

    /// The two-artifact plan most of these tests apply.
    fn pair(&self, seed: &str) -> Plan {
        plan(
            &generation_id(seed),
            vec![
                artifact("spu", SPU, &self.spu),
                artifact("uade", UADE, &self.uade),
            ],
        )
    }

    fn cached(&self, bytes: &[u8]) -> bool {
        self.queue.transport().cache().is_complete(&digest(bytes))
    }

    fn cache_path(&self, bytes: &[u8]) -> PathBuf {
        self.queue.transport().cache().path_for(&digest(bytes))
    }
}

#[test]
fn a_plan_publishes_a_generation_named_by_the_manifest_digest() {
    let fixture = Fixture::new();
    let install = fixture.install_root();
    let applier = fixture.applier(install.clone());

    let plan = plan(
        &generation_id("first"),
        vec![
            artifact("spu", SPU, &fixture.spu),
            artifact("uade", UADE, &fixture.uade),
            artifact("db", NESTED, &fixture.nested),
        ],
    );
    let generation = applier.apply(&plan, Priority::User).expect("an apply");

    assert_eq!(generation.id(), plan.generation_id);
    assert_eq!(generation.dir(), install.root().join(&plan.generation_id));
    assert_eq!(generation.artifacts().len(), 3);

    for (artifact, expected) in
        generation
            .artifacts()
            .iter()
            .zip([&fixture.spu, &fixture.uade, &fixture.nested])
    {
        let path = generation.path_of(artifact);
        assert_eq!(&fs::read(&path).expect("a published file"), expected);
        assert_eq!(artifact.sha256, digest_hex(expected));
        assert_eq!(artifact.target.as_deref(), Some("linux-x86_64"));
        assert_eq!(artifact.version.as_deref(), Some("1.2.3"));
    }
    // A manifest path with a directory in it keeps its shape.
    assert!(generation.dir().join(NESTED).is_file());

    // And it resolves the same way for a consumer that only has the id.
    assert_eq!(
        install.resolve(&plan.generation_id).as_ref(),
        Some(&generation)
    );
    assert_eq!(
        InstallRoot::new(install.root()).resolve(&plan.generation_id),
        Some(generation)
    );
}

#[test]
fn a_release_set_carrying_nothing_for_this_target_still_publishes_a_generation() {
    let fixture = Fixture::new();
    let install = fixture.install_root();
    let applier = fixture.applier(install.clone());

    let empty = plan(&generation_id("empty"), Vec::new());
    let generation = applier
        .apply(&empty, Priority::Background)
        .expect("an apply");

    assert!(generation.artifacts().is_empty());
    assert!(install.resolve(&empty.generation_id).is_some());
    assert!(fixture.server.records().is_empty());
}

#[test]
fn re_applying_an_identical_release_set_resolves_to_it_and_downloads_nothing() {
    let fixture = Fixture::new();
    let install = fixture.install_root();
    let applier = fixture.applier(install.clone());
    let plan = fixture.pair("same");

    let first = applier
        .apply(&plan, Priority::User)
        .expect("the first apply");
    assert_eq!(fixture.downloads(), 2, "one body per artifact");
    let requests = fixture.server.records().len();

    let second = applier
        .apply(&plan, Priority::User)
        .expect("the second apply");
    assert_eq!(second, first);
    assert_eq!(fixture.server.records().len(), requests);

    // Even from an applier that has never seen the generation before — the
    // install root is what answers, not the process's memory of it.
    let fresh = fixture.applier(install.clone());
    assert_eq!(
        fresh.apply(&plan, Priority::User).expect("a third apply"),
        first
    );
    assert_eq!(fixture.server.records().len(), requests);
}

#[test]
fn a_generation_is_reported_complete_exactly_once() {
    let fixture = Fixture::new();
    let applier = fixture.applier(fixture.install_root());
    let completions = applier.completions();

    let first = fixture.pair("one");
    let second = plan(
        &generation_id("two"),
        vec![artifact("db", NESTED, &fixture.nested)],
    );

    let published = applier.apply(&first, Priority::User).expect("an apply");
    applier.apply(&first, Priority::User).expect("a re-apply");
    applier
        .apply(&second, Priority::User)
        .expect("a second apply");

    assert_eq!(completions.next_completed(), Some(published));
    assert_eq!(
        completions.next_completed().map(|g| g.id().to_string()),
        Some(second.generation_id.clone())
    );
    assert_eq!(completions.next_completed(), None);

    applier
        .apply(&first, Priority::User)
        .expect("a third apply");
    assert_eq!(completions.next_completed(), None);
}

/// The queue serves a cache entry it already accepted without re-hashing it, so
/// bytes that rot after they landed reach publication unchallenged. Only the
/// re-verification of the copy stands between them and a published generation.
#[test]
fn a_cache_entry_that_rotted_after_it_landed_is_refused_at_publication() {
    let fixture = Fixture::new();
    let install = fixture.install_root();
    let applier = fixture.applier(install.clone());

    let installed = applier
        .apply(&fixture.pair("installed"), Priority::User)
        .expect("the generation already installed");

    // Same length, so the entry still reads as complete and is served from
    // disk rather than fetched again.
    let rotted: Vec<u8> = fixture.spu.iter().map(|b| !b).collect();
    fs::write(fixture.cache_path(&fixture.spu), &rotted).expect("a poisoned cache entry");
    assert!(fixture.cached(&fixture.spu), "still complete to the cache");

    let doomed = plan(
        &generation_id("doomed"),
        vec![artifact("spu", SPU, &fixture.spu)],
    );
    let err = applier.apply(&doomed, Priority::User).unwrap_err();

    assert!(
        matches!(
            &err,
            Error::Artifact { name, source: Failure::DiskDigest { .. } } if name == "spu"
        ),
        "{err}"
    );
    // Nothing unverified reached the generation, and it never became one.
    assert!(
        !install
            .root()
            .join(&doomed.generation_id)
            .join(SPU)
            .exists()
    );
    assert!(install.resolve(&doomed.generation_id).is_none());

    // The poisoned entry is out of the cache, so a retry re-fetches.
    assert!(!fixture.cached(&fixture.spu));

    // And the generation the consumer is running on still holds good bytes.
    assert_eq!(install.resolve(installed.id()).as_ref(), Some(&installed));
    assert_eq!(fs::read(installed.dir().join(SPU)).unwrap(), fixture.spu);
}

#[test]
fn an_artifact_whose_bytes_are_not_what_the_manifest_named_is_quarantined() {
    let fixture = Fixture::new();
    let install = fixture.install_root();
    let applier = fixture.applier(install.clone());

    let installed = applier
        .apply(&fixture.pair("installed"), Priority::User)
        .expect("the generation already installed");

    let doomed = plan(
        &generation_id("doomed"),
        vec![artifact("spu", SPU, &fixture.spu), fixture.impostor()],
    );

    let err = applier.apply(&doomed, Priority::User).unwrap_err();
    assert!(
        matches!(&err, Error::Artifact { name, .. } if name == "impostor"),
        "{err}"
    );

    // The installed generation is untouched and still resolves.
    assert_eq!(install.resolve(installed.id()).as_ref(), Some(&installed));
    assert_eq!(fs::read(installed.dir().join(SPU)).unwrap(), fixture.spu);

    // The half-applied one never became a generation.
    assert!(install.resolve(&doomed.generation_id).is_none());
    assert_eq!(
        install.generations().unwrap().len(),
        1,
        "only the installed generation resolves"
    );

    // The bytes that failed are out of the cache, so the next attempt cannot
    // take a hit on them.
    assert!(!fixture.cached(&fixture.claimed));
    assert!(!fixture.cached(&fixture.impostor));
    assert!(fixture.cached(&fixture.spu));
}

/// One worker drains in queue order, so the good artifact lands before the
/// bad one is even started: the generation directory holds a file, and the
/// record that would make it resolvable was never written.
#[test]
fn files_that_landed_before_the_failure_never_add_up_to_a_generation() {
    let fixture = Fixture::with_workers(1);
    let install = fixture.install_root();
    let applier = fixture.applier(install.clone());

    let installed = applier
        .apply(&fixture.pair("installed"), Priority::User)
        .expect("the generation already installed");

    let doomed = plan(
        &generation_id("doomed"),
        vec![artifact("db", NESTED, &fixture.nested), fixture.impostor()],
    );
    applier.apply(&doomed, Priority::User).unwrap_err();

    let half_applied = install.root().join(&doomed.generation_id);
    assert_eq!(
        fs::read(half_applied.join(NESTED)).expect("the artifact that landed"),
        fixture.nested
    );
    assert!(install.resolve(&doomed.generation_id).is_none());
    assert!(!half_applied.join(IMPOSTOR).exists());

    // And the generation the consumer is running on is untouched.
    assert_eq!(install.resolve(installed.id()).as_ref(), Some(&installed));
    assert_eq!(install.generations().unwrap().len(), 1);
}

#[test]
fn a_failed_apply_leaves_the_queue_with_every_slot_free() {
    let fixture = Fixture::new();
    let applier = fixture.applier(fixture.install_root());

    let doomed = plan(&generation_id("doomed"), vec![fixture.impostor()]);
    applier.apply(&doomed, Priority::User).unwrap_err();

    // Nothing is left holding a slot: a full queue's worth of work still fits.
    let queue = Arc::clone(&fixture.queue);
    let ids: Vec<_> = (0..QUEUE_MAX)
        .map(|_| {
            queue.queue(Request {
                url: fixture.server.url(&format!("/{SPU}")),
                digest: digest(&fixture.spu),
                expected_size: None,
                priority: Priority::Background,
            })
        })
        .collect();
    assert!(ids.iter().all(Option::is_some), "a slot was leaked");
    for id in ids.into_iter().flatten() {
        let _ = queue.cancel(id);
    }
}

/// A release set can name more artifacts than the queue has slots, so the
/// apply has to keep feeding it as transfers settle rather than queueing the
/// plan in one go.
#[test]
fn a_plan_larger_than_the_queue_still_publishes_every_artifact() {
    let count = QUEUE_MAX + 5;
    let bodies: Vec<Vec<u8>> = (0..count)
        .map(|i| body_bytes(0x40 + u8::try_from(i).expect("a small fixture count")))
        .collect();
    let paths: Vec<String> = (0..count).map(|i| format!("bulk-{i}.tar.zst")).collect();

    let mut routes = HashMap::new();
    for (path, bytes) in paths.iter().zip(&bodies) {
        routes.insert(
            format!("/{path}"),
            Body::instant(bytes.clone(), &format!("\"{path}\"")),
        );
    }

    let dir = tempfile::tempdir().expect("a scratch directory");
    let server = FixtureServer::start(routes);
    let queue = Arc::new(TransferQueue::with_config(
        Transport::new(dir.path().join("cache")),
        Config {
            workers: 4,
            ..Config::default()
        },
    ));
    let install = InstallRoot::new(dir.path().join("install"));
    let applier = Applier::new(Arc::clone(&queue), install.clone(), &server.url(""));

    let artifacts: Vec<Artifact> = paths
        .iter()
        .zip(&bodies)
        .enumerate()
        .map(|(i, (path, bytes))| artifact(&format!("bulk-{i}"), path, bytes))
        .collect();
    let bulk = plan(&generation_id("bulk"), artifacts);
    let generation = applier
        .apply(&bulk, Priority::Background)
        .expect("an apply");

    assert_eq!(generation.artifacts().len(), count);
    // In manifest order, and every one of them the bytes it named.
    for (installed, (path, bytes)) in generation.artifacts().iter().zip(paths.iter().zip(&bodies)) {
        assert_eq!(&installed.path, path);
        assert_eq!(&fs::read(generation.path_of(installed)).unwrap(), bytes);
    }
    assert_eq!(install.resolve(&bulk.generation_id), Some(generation));
}

#[test]
fn retention_keeps_what_the_caller_names_and_removes_the_rest() {
    let fixture = Fixture::new();
    let install = fixture.install_root();
    let applier = fixture.applier(install.clone());

    let old = applier
        .apply(&fixture.pair("old"), Priority::Background)
        .expect("the first generation");
    let new = applier
        .apply(
            &plan(
                &generation_id("new"),
                vec![artifact("db", NESTED, &fixture.nested)],
            ),
            Priority::Background,
        )
        .expect("the second generation");

    assert_eq!(install.generations().unwrap().len(), 2);
    let removed = install.retain(&[new.id().to_string()]).expect("retention");

    assert_eq!(removed, [old.id().to_string()]);
    assert!(!old.dir().exists());
    assert!(install.resolve(old.id()).is_none());
    assert_eq!(install.resolve(new.id()).as_ref(), Some(&new));
    assert!(new.dir().join(NESTED).is_file());
}
