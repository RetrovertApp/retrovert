//! Checking a channel: what it offers, what applying it would cost, and the
//! record every attempt leaves behind.
//!
//! Everything here runs against a channel published into a temporary directory.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::channel::{Channel, Clock, NetworkTime, TrustStore};
use crate::check::{CheckLog, Checker, Conclusion, Outcome, Plan, Skipped};
use crate::queue::{Priority, Request, State, TransferQueue};
use crate::testing::fixture_channel::{FixtureChannel, artifact, manifest_of};
use crate::testing::fixture_server::{Body, FixtureServer};
use crate::transport::{ArtifactDigest, Cache, Transport};
use jiff::Timestamp;
use serde_json::Value;
use sha2::{Digest, Sha256};
use sigstore_tuf::transport::Repository;
use tempfile::TempDir;

/// A fixed instant, so a fixture is datable to whatever a test needs.
fn now() -> Timestamp {
    "2026-08-15T12:00:00Z".parse().unwrap()
}

const REVISION: &str = "abc1234";
const LINUX: &str = "linux-x86_64";
const WINDOWS: &str = "windows-x86_64";

/// A clock standing in for the network.
struct At(Timestamp);

impl Clock for At {
    fn network_time(&self) -> Option<NetworkTime> {
        Some(NetworkTime::new(self.0))
    }
}

/// A network that cannot be asked what time it is.
struct Unreachable;

impl Clock for Unreachable {
    fn network_time(&self) -> Option<NetworkTime> {
        None
    }
}

/// A published channel and the three directories a consumer keeps beside it.
struct Fixture {
    channel: FixtureChannel,
    trust: TempDir,
    state: TempDir,
    cache: TempDir,
}

impl Fixture {
    fn new() -> Self {
        Self {
            channel: FixtureChannel::init(now()),
            trust: TempDir::new().unwrap(),
            state: TempDir::new().unwrap(),
            cache: TempDir::new().unwrap(),
        }
    }

    /// Publish `artifacts` as release set `version`, returning the generation id.
    fn publish(&mut self, version: u64, artifacts: &[Value]) -> String {
        let bytes = manifest_of(version, REVISION, artifacts);
        self.channel.publish(&bytes, now())
    }

    fn checker(&self, target: Option<&str>) -> Checker {
        self.checker_over(self.channel.source(), target, Arc::new(At(now())))
    }

    fn checker_over(
        &self,
        source: Arc<dyn Repository>,
        target: Option<&str>,
        clock: Arc<dyn Clock>,
    ) -> Checker {
        Checker::new(
            Channel::new(
                source,
                clock,
                self.channel.root(),
                TrustStore::new(self.trust.path()),
            ),
            target.map(str::to_string),
            self.cache(),
            self.log(),
        )
    }

    fn cache(&self) -> Cache {
        Cache::new(self.cache.path())
    }

    fn log(&self) -> CheckLog {
        CheckLog::new(self.state.path())
    }

    /// The cache entries present right now.
    fn cached_entries(&self) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(self.cache.path())
            .expect("a readable cache directory")
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }
}

fn plan_of(outcome: Outcome) -> Plan {
    match outcome {
        Outcome::UpdateAvailable(plan) => plan,
        other => panic!("expected an update, got {other:?}"),
    }
}

fn names(plan: &Plan) -> Vec<&str> {
    plan.artifacts
        .iter()
        .map(|artifact| artifact.name.as_str())
        .collect()
}

fn one_artifact() -> [Value; 1] {
    [artifact("spu", Some(LINUX), &"ab".repeat(32), 42, REVISION)]
}

#[test]
fn a_generation_this_client_does_not_have_is_an_update_carrying_a_plan() {
    let mut fixture = Fixture::new();
    let generation_id = fixture.publish(1, &one_artifact());

    let plan = plan_of(fixture.checker(Some(LINUX)).check(None).unwrap());

    assert_eq!(plan.generation_id, generation_id);
    assert_eq!(plan.artifact_count(), 1);
    assert_eq!(plan.artifacts[0].name, "spu");
    assert_eq!(plan.total_bytes, 42);
    assert_eq!(plan.cached_bytes, 0);
    assert_eq!(plan.bytes_to_fetch(), 42);
}

#[test]
fn the_generation_already_installed_is_up_to_date() {
    let mut fixture = Fixture::new();
    let generation_id = fixture.publish(1, &one_artifact());

    let outcome = fixture
        .checker(Some(LINUX))
        .check(Some(&generation_id))
        .unwrap();

    assert!(
        matches!(outcome, Outcome::UpToDate { generation_id: ref reported } if *reported == generation_id),
        "{outcome:?}"
    );
}

#[test]
fn a_republished_channel_moves_an_up_to_date_client_back_to_an_update() {
    let mut fixture = Fixture::new();
    let first = fixture.publish(1, &one_artifact());
    let second = fixture.publish(2, &one_artifact());
    assert_ne!(first, second);

    let plan = plan_of(fixture.checker(Some(LINUX)).check(Some(&first)).unwrap());

    assert_eq!(plan.generation_id, second);
}

#[test]
fn a_check_without_network_time_is_skipped_with_a_reason() {
    let mut fixture = Fixture::new();
    fixture.publish(1, &one_artifact());

    let outcome = fixture
        .checker_over(fixture.channel.source(), Some(LINUX), Arc::new(Unreachable))
        .check(None)
        .unwrap();

    assert!(
        matches!(outcome, Outcome::Skipped(Skipped::NoNetworkTime)),
        "{outcome:?}"
    );
    assert_eq!(
        fixture.log().last().unwrap().unwrap().conclusion,
        Conclusion::Skipped
    );
}

#[test]
fn an_untargeted_artifact_is_taken_by_every_consumer() {
    let mut fixture = Fixture::new();
    fixture.publish(
        1,
        &[
            artifact("spu", Some(LINUX), &"ab".repeat(32), 10, REVISION),
            artifact("db", None, &"cd".repeat(32), 30, REVISION),
        ],
    );

    for target in [Some(LINUX), Some(WINDOWS), None] {
        let plan = plan_of(fixture.checker(target).check(None).unwrap());
        assert!(
            names(&plan).contains(&"db"),
            "{target:?} missed the database"
        );
    }
}

#[test]
fn an_artifact_naming_another_target_is_excluded() {
    let mut fixture = Fixture::new();
    fixture.publish(
        1,
        &[
            artifact("spu", Some(LINUX), &"ab".repeat(32), 10, REVISION),
            artifact("spu", Some(WINDOWS), &"cd".repeat(32), 20, REVISION),
            artifact("db", None, &"ef".repeat(32), 30, REVISION),
        ],
    );

    let linux = plan_of(fixture.checker(Some(LINUX)).check(None).unwrap());
    assert_eq!(names(&linux), ["spu", "db"]);
    assert_eq!(linux.artifacts[0].target.as_deref(), Some(LINUX));
    assert_eq!(linux.total_bytes, 40);

    let unconfigured = plan_of(fixture.checker(None).check(None).unwrap());
    assert_eq!(names(&unconfigured), ["db"]);
    assert_eq!(unconfigured.total_bytes, 30);

    let stranger = plan_of(fixture.checker(Some("macos-aarch64")).check(None).unwrap());
    assert_eq!(names(&stranger), ["db"]);
}

#[test]
fn a_release_set_holding_nothing_for_this_target_is_still_an_update() {
    let mut fixture = Fixture::new();
    let generation_id = fixture.publish(
        1,
        &[artifact(
            "spu",
            Some(WINDOWS),
            &"ab".repeat(32),
            20,
            REVISION,
        )],
    );

    let plan = plan_of(fixture.checker(Some(LINUX)).check(None).unwrap());

    assert_eq!(plan.generation_id, generation_id);
    assert_eq!(plan.artifact_count(), 0);
    assert_eq!(plan.bytes_to_fetch(), 0);
}

#[test]
fn a_check_fetches_no_artifact() {
    let mut fixture = Fixture::new();
    fixture.publish(
        1,
        &[
            artifact("spu", Some(LINUX), &"ab".repeat(32), 10, REVISION),
            artifact("db", None, &"cd".repeat(32), 30, REVISION),
        ],
    );
    assert_eq!(fixture.cached_entries(), Vec::<String>::new());

    let plan = plan_of(fixture.checker(Some(LINUX)).check(None).unwrap());

    assert_eq!(plan.artifact_count(), 2);
    assert_eq!(
        fixture.cached_entries(),
        Vec::<String>::new(),
        "a check prices artifacts; it never pulls one"
    );
}

#[test]
fn every_check_appends_to_a_record_that_survives_a_restart() {
    let mut fixture = Fixture::new();
    fixture.publish(1, &one_artifact());

    fixture
        .checker_over(fixture.channel.source(), Some(LINUX), Arc::new(Unreachable))
        .check(None)
        .unwrap();
    fixture.checker(Some(LINUX)).check(None).unwrap();

    // A checker built fresh over the same state directory, as a restarted
    // consumer builds one.
    let entries = CheckLog::new(fixture.state.path()).entries().unwrap();
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].conclusion, Conclusion::Skipped);
    assert_eq!(entries[1].conclusion, Conclusion::UpdateAvailable);
    assert!(entries.iter().all(|entry| entry.error.is_none()));
    assert!(entries[0].at <= entries[1].at);
}

#[test]
fn a_failed_check_is_recorded_with_the_error_that_failed_it() {
    let mut fixture = Fixture::new();
    let bytes = manifest_of(1, REVISION, &one_artifact());
    let generation_id = fixture.channel.publish(&bytes, now());

    // Same length, different content, so the refusal comes from the digest.
    let mut forged = bytes.clone();
    let last = forged.len() - 2;
    forged[last] = if forged[last] == b' ' { b'\t' } else { b' ' };

    let source = fixture
        .channel
        .tampered(&format!("{generation_id}.manifest.json"), forged);
    let err = fixture
        .checker_over(source, Some(LINUX), Arc::new(At(now())))
        .check(None)
        .unwrap_err();

    let recorded = fixture.log().last().unwrap().unwrap();
    assert_eq!(recorded.conclusion, Conclusion::Failed);
    assert_eq!(recorded.error.as_deref(), Some(err.to_string().as_str()));
}

#[test]
fn an_artifact_already_in_the_cache_is_priced_as_cached() {
    const BODY: &str = "/spu.tar.zst";
    let body: Vec<u8> = (0..4096u32).map(|i| i.to_le_bytes()[0]).collect();
    let sha256 = hex::encode(Sha256::digest(&body));
    let size = u64::try_from(body.len()).unwrap();

    let mut fixture = Fixture::new();
    fixture.publish(
        1,
        &[
            artifact("spu", Some(LINUX), &sha256, size, REVISION),
            artifact("db", None, &"cd".repeat(32), 30, REVISION),
        ],
    );

    let server = FixtureServer::start(HashMap::from([(
        BODY.to_string(),
        Body::instant(body, "\"spu\""),
    )]));
    let queue = TransferQueue::new(Transport::new(fixture.cache.path()));
    let id = queue
        .queue(Request {
            url: server.url(BODY),
            digest: ArtifactDigest::from_hex(&sha256).unwrap(),
            expected_size: Some(size),
            priority: Priority::User,
        })
        .expect("a free queue slot");

    let deadline = Instant::now() + Duration::from_secs(60);
    while queue.state(id) != State::Complete {
        assert!(
            Instant::now() < deadline,
            "the fixture transfer never landed"
        );
        std::thread::sleep(Duration::from_millis(20));
    }

    let plan = plan_of(fixture.checker(Some(LINUX)).check(None).unwrap());

    assert_eq!(plan.total_bytes, size + 30);
    assert_eq!(
        plan.cached_bytes, size,
        "the cached artifact is priced at nil"
    );
    assert_eq!(plan.bytes_to_fetch(), 30);
}
