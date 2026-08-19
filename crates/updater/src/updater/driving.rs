//! Driving the facade the way a consumer does: tick, poll, apply.
//!
//! The channel is a fixture read straight from disk and the artifacts come off
//! an in-process HTTP server, so everything here runs wherever `cargo test`
//! runs.

use std::collections::HashMap;
use std::num::NonZeroUsize;
use std::sync::Arc;
use std::time::{Duration, Instant};

use jiff::Timestamp;
use serde_json::Value;
use tempfile::TempDir;

use super::{Phase, Priority, Updater, UpdaterConfig};
use crate::channel::{Channel, Clock, NetworkTime, TrustStore};
use crate::testing::fixture_channel::{FixtureChannel, Payload, manifest_of, payload};
use crate::testing::fixture_server::{Body, FixtureServer};
use crate::updater::config::{ChannelConfig, WorkerConfig};

const REVISION: &str = "abc1234";
const LINUX: &str = "linux-x86_64";

/// How long a test waits for the updater's thread before calling it stuck.
const SETTLE: Duration = Duration::from_secs(30);

/// A fixed instant, so the fixture channel is datable.
fn now() -> Timestamp {
    "2026-08-15T12:00:00Z".parse().unwrap()
}

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

fn plugin(name: &str, seed: u8, len: u16) -> Payload {
    payload(name, Some(LINUX), REVISION, seed, len)
}

/// A published channel, a host serving its artifacts, and the directories a
/// consumer keeps beside them.
struct Fixture {
    channel: FixtureChannel,
    server: FixtureServer,
    dir: TempDir,
    payloads: Vec<Payload>,
    generation_id: String,
    workers: NonZeroUsize,
}

impl Fixture {
    /// One 4 KiB artifact, served as fast as the socket takes it.
    fn instant() -> Self {
        Self::new(vec![plugin("spu", 1, 4096)], None, 1)
    }

    /// Two 16 KiB artifacts served in 512-byte pieces, on two workers: slow
    /// enough that a test can act mid-transfer, and wide enough that what it
    /// does has to reach more than one of them.
    fn throttled() -> Self {
        Self::new(
            vec![plugin("spu", 1, 16 * 1024), plugin("uade", 2, 16 * 1024)],
            Some((512, Duration::from_millis(10))),
            2,
        )
    }

    fn new(payloads: Vec<Payload>, throttle: Option<(usize, Duration)>, workers: usize) -> Self {
        let mut routes = HashMap::new();
        for payload in &payloads {
            let body = match throttle {
                Some((piece, delay)) => {
                    Body::throttled(payload.bytes.clone(), "\"etag\"", piece, delay)
                }
                None => Body::instant(payload.bytes.clone(), "\"etag\""),
            };
            routes.insert(format!("/{}", payload.path), body);
        }

        let mut channel = FixtureChannel::init(now());
        let entries: Vec<Value> = payloads.iter().map(|p| p.entry.clone()).collect();
        let generation_id = channel.publish(&manifest_of(1, REVISION, &entries), now());

        Self {
            channel,
            server: FixtureServer::start(routes),
            dir: TempDir::new().expect("a scratch directory"),
            payloads,
            generation_id,
            workers: NonZeroUsize::new(workers).expect("at least one worker"),
        }
    }

    fn config(&self, auto_apply: bool) -> UpdaterConfig {
        UpdaterConfig {
            install_root: self.dir.path().join("install"),
            cache_dir: self.dir.path().join("cache"),
            trust_state_dir: self.dir.path().join("state"),
            channel: ChannelConfig {
                metadata_base_url: "https://example.invalid".to_string(),
                artifact_base_url: self.server.url(""),
            },
            embedded_root: self.channel.root(),
            target: Some(LINUX.to_string()),
            check_interval: Duration::from_secs(3600),
            auto_apply,
            workers: WorkerConfig {
                count: self.workers,
                ..WorkerConfig::default()
            },
        }
    }

    fn updater(&self, auto_apply: bool) -> Updater {
        self.updater_timed(auto_apply, Arc::new(At(now())))
    }

    fn updater_timed(&self, auto_apply: bool, clock: Arc<dyn Clock>) -> Updater {
        let config = self.config(auto_apply);
        let trust = TrustStore::new(&config.trust_state_dir);
        let channel = Channel::new(
            self.channel.source(),
            clock,
            config.embedded_root.clone(),
            trust,
        );
        Updater::over(config, channel)
    }

    fn requests_for_artifacts(&self) -> usize {
        self.payloads
            .iter()
            .map(|payload| self.server.request_count(&format!("/{}", payload.path)))
            .sum()
    }

    /// How many of the plan's artifacts have bytes in the cache, which is how
    /// a test knows a transfer is genuinely under way rather than queued.
    fn transfers_under_way(&self) -> usize {
        std::fs::read_dir(self.dir.path().join("cache"))
            .into_iter()
            .flatten()
            .flatten()
            .filter(|entry| !entry.file_name().to_string_lossy().ends_with(".meta"))
            .filter(|entry| entry.metadata().is_ok_and(|file| file.len() > 0))
            .count()
    }

    /// Wait until every artifact of the plan is transferring, so a sweep over
    /// the queue has more than one entry to reach.
    fn wait_for_every_transfer(&self, updater: &Updater) -> f32 {
        let deadline = Instant::now() + SETTLE;
        loop {
            let status = updater.poll();
            if self.transfers_under_way() == self.payloads.len() {
                return status.progress;
            }
            assert!(
                status.phase != Phase::Idle,
                "the updater stopped before every artifact moved: {status:?}"
            );
            assert!(Instant::now() < deadline, "not every transfer started");
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}

/// Wait for the updater's thread to finish whatever it is doing.
fn settle(updater: &Updater) {
    let deadline = Instant::now() + SETTLE;
    while updater.poll().phase != Phase::Idle {
        assert!(Instant::now() < deadline, "the updater never went idle");
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// Wait for an apply to have some of its plan on disk, so a test can act on a
/// transfer that is genuinely running.
fn wait_for_progress(updater: &Updater) -> f32 {
    let deadline = Instant::now() + SETTLE;
    loop {
        let status = updater.poll();
        if status.progress > 0.0 {
            return status.progress;
        }
        assert!(
            status.phase != Phase::Idle,
            "the updater stopped before it transferred anything: {status:?}"
        );
        assert!(Instant::now() < deadline, "no bytes ever arrived");
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn a_plaintext_artifact_base_is_refused_at_construction() {
    let fixture = Fixture::instant();
    let mut config = fixture.config(false);
    // The fixture host is plaintext, which `Updater::over` tolerates as a
    // test seam; the production constructor must not.
    assert!(config.channel.artifact_base_url.starts_with("http://"));

    let err = Updater::new(config.clone()).unwrap_err();
    assert!(matches!(err, super::Error::Insecure(_)), "{err}");

    // With the artifact base secured, the metadata base is checked the same
    // way further down the constructor.
    config.channel.artifact_base_url = "https://example.invalid/artifacts".to_string();
    config.channel.metadata_base_url = "http://example.invalid/metadata".to_string();
    let err = Updater::new(config).unwrap_err();
    assert!(matches!(err, super::Error::Channel(_)), "{err}");
}

#[test]
fn trust_state_inside_the_cache_is_refused_at_construction() {
    let fixture = Fixture::instant();
    let mut config = fixture.config(false);
    config.channel.artifact_base_url = "https://example.invalid/artifacts".to_string();
    config.trust_state_dir = config.cache_dir.join("trust");

    let err = Updater::new(config).unwrap_err();
    assert!(
        matches!(err, super::Error::TrustStateInCache { .. }),
        "{err}"
    );
}

#[test]
fn a_tick_checks_once_and_then_waits_out_the_interval() {
    let fixture = Fixture::instant();
    let updater = fixture.updater(false);

    assert!(updater.tick(), "the first tick is due");
    settle(&updater);
    assert!(!updater.tick(), "the interval has not passed");

    assert_eq!(
        updater.poll().available.expect("a plan").generation_id,
        fixture.generation_id
    );
}

#[test]
fn a_check_now_runs_whatever_the_interval_says() {
    let fixture = Fixture::instant();
    let updater = fixture.updater(false);

    assert!(updater.tick());
    settle(&updater);
    assert!(!updater.tick());

    assert!(updater.check_now(Priority::User), "check-now ignores it");
    settle(&updater);
    assert!(updater.poll().available.is_some());
}

#[test]
fn a_poll_reports_the_phase_the_progress_and_the_last_check() {
    let fixture = Fixture::instant();
    let updater = fixture.updater(false);

    let before = updater.poll();
    assert_eq!(before.phase, Phase::Idle);
    assert!((before.progress - 0.0).abs() < f32::EPSILON);
    assert_eq!(before.last_check, None);

    updater.check_now(Priority::User);
    settle(&updater);

    let after = updater.poll();
    assert_eq!(after.phase, Phase::Idle);
    assert_eq!(
        after.last_check.expect("a recorded check").conclusion,
        crate::check::Conclusion::UpdateAvailable
    );
}

#[test]
fn the_record_of_an_earlier_run_is_there_from_the_first_poll() {
    let fixture = Fixture::instant();
    {
        let updater = fixture.updater(false);
        updater.check_now(Priority::User);
        settle(&updater);
    }

    let restarted = fixture.updater(false);
    assert_eq!(
        restarted
            .poll()
            .last_check
            .expect("the record outlives the process")
            .conclusion,
        crate::check::Conclusion::UpdateAvailable
    );
}

#[test]
fn a_check_that_could_not_run_backs_the_schedule_off_past_its_interval() {
    let fixture = Fixture::instant();
    let updater = fixture.updater_timed(false, Arc::new(Unreachable));

    assert!(updater.tick());
    settle(&updater);

    let status = updater.poll();
    assert_eq!(
        status.last_check.expect("a recorded check").conclusion,
        crate::check::Conclusion::Skipped
    );
    assert_eq!(status.available, None);
    assert!(!updater.tick(), "a skipped check still costs the interval");
}

#[test]
fn with_automatic_apply_off_nothing_is_fetched_until_the_caller_applies() {
    let fixture = Fixture::instant();
    let updater = fixture.updater(false);

    updater.check_now(Priority::User);
    settle(&updater);

    let plan = updater.poll().available.expect("a plan");
    assert_eq!(
        fixture.requests_for_artifacts(),
        0,
        "a plan fetches nothing"
    );
    assert_eq!(updater.next_completed(), None);

    assert!(updater.apply(&plan, Priority::User));
    settle(&updater);

    let generation = updater.next_completed().expect("a published generation");
    assert_eq!(generation.id(), fixture.generation_id);
    assert!(fixture.requests_for_artifacts() > 0, "the apply fetched it");
    assert_eq!(updater.poll().available, None);
    assert_eq!(updater.next_completed(), None, "offered exactly once");
}

#[test]
fn automatic_apply_carries_a_successful_check_into_an_apply() {
    let fixture = Fixture::instant();
    let updater = fixture.updater(true);

    assert!(updater.tick());
    settle(&updater);

    let generation = updater.next_completed().expect("a published generation");
    assert_eq!(generation.id(), fixture.generation_id);
    assert_eq!(generation.artifacts().len(), fixture.payloads.len());
    assert_eq!(
        std::fs::read(generation.path_of(&generation.artifacts()[0])).unwrap(),
        fixture.payloads[0].bytes
    );
}

#[test]
fn generations_are_published_under_the_install_root_and_collected_on_request() {
    let fixture = Fixture::instant();
    let updater = fixture.updater(true);

    updater.tick();
    settle(&updater);

    let generations = updater.generations().unwrap();
    assert_eq!(generations.len(), 1);
    assert!(generations[0].dir().starts_with(updater.generations_dir()));
    assert!(updater.generations_dir().starts_with(fixture.dir.path()));

    assert_eq!(
        updater
            .retain(std::slice::from_ref(&fixture.generation_id))
            .unwrap(),
        Vec::<String>::new()
    );
    assert_eq!(updater.retain(&[]).unwrap(), vec![fixture.generation_id]);
    assert_eq!(updater.generations().unwrap().len(), 0);
}

#[test]
fn a_second_check_finds_the_client_up_to_date_on_what_it_applied() {
    let fixture = Fixture::instant();
    let updater = fixture.updater(true);

    updater.tick();
    settle(&updater);
    updater.check_now(Priority::Background);
    settle(&updater);

    let status = updater.poll();
    assert_eq!(status.available, None);
    assert_eq!(
        status.last_check.expect("a recorded check").conclusion,
        crate::check::Conclusion::UpToDate
    );
}

#[test]
fn a_pause_stops_a_transfer_and_a_resume_carries_it_to_a_generation() {
    let fixture = Fixture::throttled();
    let updater = fixture.updater(true);

    updater.tick();
    let running = fixture.wait_for_every_transfer(&updater);
    updater.pause();

    // Long enough that either transfer, left running, would have served enough
    // pieces to move progress well past this; a paused one lands at most the
    // piece already in flight. So the margin holds only if the pause reached
    // both.
    std::thread::sleep(Duration::from_millis(200));
    let paused = updater.poll();
    assert_eq!(paused.phase, Phase::Applying);
    assert!(
        paused.progress - running < 0.1,
        "a paused transfer kept going: {running} to {}",
        paused.progress
    );

    updater.resume();
    settle(&updater);
    assert_eq!(
        updater.next_completed().expect("a generation").id(),
        fixture.generation_id
    );
}

#[test]
fn a_cancel_ends_the_apply_and_leaves_the_install_root_as_it_was() {
    let fixture = Fixture::throttled();
    let updater = fixture.updater(true);

    updater.tick();
    fixture.wait_for_every_transfer(&updater);
    updater.cancel();
    settle(&updater);

    let status = updater.poll();
    assert!(status.last_error.is_some(), "{status:?}");
    assert_eq!(updater.next_completed(), None);
    assert_eq!(updater.generations().unwrap().len(), 0);
}

#[test]
fn only_one_check_or_apply_runs_at_a_time() {
    let fixture = Fixture::throttled();
    let updater = fixture.updater(true);

    assert!(updater.tick());
    wait_for_progress(&updater);
    assert!(!updater.check_now(Priority::User), "an apply is running");
    assert!(
        !updater.apply(
            &crate::check::Plan {
                generation_id: "unused".to_string(),
                artifacts: Vec::new(),
                total_bytes: 0,
                cached_bytes: 0,
            },
            Priority::User
        ),
        "an apply is running"
    );

    updater.cancel();
    settle(&updater);
    assert!(
        updater.check_now(Priority::User),
        "the thread is free again"
    );
}

#[test]
fn a_job_that_panics_leaves_the_updater_taking_work() {
    let fixture = Fixture::instant();
    let updater = fixture.updater(false);

    assert!(updater.run_panicking_job());
    settle(&updater);

    let status = updater.poll();
    assert_eq!(status.phase, Phase::Idle);
    assert!(status.last_error.is_some(), "{status:?}");

    assert!(
        updater.check_now(Priority::User),
        "the thread is free again"
    );
    settle(&updater);
    assert!(updater.poll().available.is_some());
}
