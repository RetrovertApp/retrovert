//! The publisher and the client meeting over one channel.
//!
//! A release set is published, then checked, fetched, verified, and installed
//! through the facade, with nothing crossing between the two halves but the
//! channel's bytes and the trust anchor. The publisher here is
//! [`crate::testing::fixture_channel`], for the reasons its own documentation
//! gives; what the `retrovert-publish` CLI serves is what the opt-in live round
//! trip at the end of this file walks.
//!
//! The refusals carry more weight than the happy path. A stale or hostile
//! mirror does not serve a corrupt chain; it serves a validly signed, unexpired,
//! *older* one, and the version floor is the only thing that answers it.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use jiff::tz::TimeZone;
use jiff::{Span, Timestamp};
use serde_json::Value;
use sha2::{Digest, Sha256};
use sigstore_tuf::Repository;
use tempfile::TempDir;

use super::{Phase, Priority, Updater, UpdaterConfig};
use crate::channel::{Channel, Clock, NetworkTime, TrustStore};
use crate::check::Conclusion;
use crate::testing::fixture_channel::{FixtureChannel, Payload, manifest_of, payload};
use crate::testing::fixture_server::{Body, FixtureServer};
use crate::transport::{ArtifactDigest, Cache};
use crate::updater::config::{ChannelConfig, WorkerConfig};

const REVISION: &str = "abc1234";
const NEXT_REVISION: &str = "def5678";
const LINUX: &str = "linux-x86_64";

/// How long a test waits for the updater's thread before calling it stuck.
const SETTLE: Duration = Duration::from_secs(30);

/// A live channel is on the far side of a network and may serve real payloads,
/// so the opt-in round trip waits far longer than a loopback one.
const LIVE_SETTLE: Duration = Duration::from_secs(600);

/// A fixed instant, so the fixture channel is datable.
fn now() -> Timestamp {
    "2026-08-15T12:00:00Z".parse().unwrap()
}

/// `count` days on from [`now`], in UTC — the same arithmetic the expiry policy
/// signs with.
fn days_on(count: i64) -> Timestamp {
    now()
        .to_zoned(TimeZone::UTC)
        .checked_add(Span::new().days(count))
        .unwrap()
        .timestamp()
}

/// A clock standing in for the network.
struct At(Timestamp);

impl Clock for At {
    fn network_time(&self) -> Option<NetworkTime> {
        Some(NetworkTime::new(self.0))
    }
}

/// One release set: the artifacts, the manifest naming them, and the host the
/// release serves them from.
struct Release {
    payloads: Vec<Payload>,
    manifest: Vec<u8>,
    host: FixtureServer,
}

impl Release {
    /// A set of `names` at `revision`, published as manifest `version` and
    /// served as the manifest describes it.
    fn new(version: u64, revision: &str, names: &[&str]) -> Self {
        Self::build(version, revision, names, false)
    }

    /// The same set, with every artifact served as bytes the manifest never
    /// named — a mirror handing back something else.
    fn tampered(version: u64, revision: &str, names: &[&str]) -> Self {
        Self::build(version, revision, names, true)
    }

    fn build(version: u64, revision: &str, names: &[&str], tamper: bool) -> Self {
        let payloads: Vec<Payload> = names
            .iter()
            .enumerate()
            .map(|(index, name)| {
                // Seeded by version as well as by position: two sets sharing
                // artifact bytes would share a cache entry, and the later one
                // would be resolved from it without ever reaching the wire.
                // Widened before narrowing, so a fixture whose seeds would
                // collide is refused rather than wrapped into one.
                let index = u64::try_from(index).expect("a small fixture set");
                let seed = u8::try_from(version * 16 + index + 1)
                    .expect("a fixture whose seeds cannot collide");
                payload(name, Some(LINUX), revision, seed, 4096)
            })
            .collect();

        let routes: HashMap<String, Body> = payloads
            .iter()
            .map(|artifact| {
                let mut bytes = artifact.bytes.clone();
                if tamper {
                    // Same length, so the refusal has to come from the digest
                    // rather than from the size the request named.
                    bytes[0] = bytes[0].wrapping_add(1);
                }
                (
                    format!("/{}", artifact.path),
                    Body::instant(bytes, "\"etag\""),
                )
            })
            .collect();

        let entries: Vec<Value> = payloads
            .iter()
            .map(|artifact| artifact.entry.clone())
            .collect();

        Self {
            manifest: manifest_of(version, revision, &entries),
            payloads,
            host: FixtureServer::start(routes),
        }
    }

    /// The digest of the published manifest, which is what a generation of this
    /// set is named by.
    fn generation_id(&self) -> String {
        hex::encode(Sha256::digest(&self.manifest))
    }

    /// How many artifact requests the release's host has answered.
    fn fetches(&self) -> usize {
        self.host.records().len()
    }

    fn total_bytes(&self) -> u64 {
        self.payloads
            .iter()
            .map(|artifact| artifact.bytes.len() as u64)
            .sum()
    }

    fn paths(&self) -> Vec<&str> {
        self.payloads
            .iter()
            .map(|artifact| artifact.path.as_str())
            .collect()
    }
}

/// The directories a consumer keeps, which outlive any one updater over them.
struct Client {
    dir: TempDir,
}

impl Client {
    fn new() -> Self {
        Self {
            dir: TempDir::new().expect("a scratch directory"),
        }
    }

    /// An updater following `channel` and fetching `release`'s artifacts.
    fn updater(
        &self,
        channel: &FixtureChannel,
        release: &Release,
        auto_apply: bool,
        at: Timestamp,
    ) -> Updater {
        self.updater_over(
            channel.source(),
            channel.root(),
            release,
            auto_apply,
            Arc::new(At(at)),
        )
    }

    /// The same, over a source and a clock the caller chose — how a test points
    /// one client at a mirror the channel has moved on from.
    fn updater_over(
        &self,
        source: Arc<dyn Repository>,
        embedded_root: Vec<u8>,
        release: &Release,
        auto_apply: bool,
        clock: Arc<dyn Clock>,
    ) -> Updater {
        let config = UpdaterConfig {
            install_root: self.dir.path().join("install"),
            cache_dir: self.cache_dir(),
            trust_state_dir: self.trust_state_dir(),
            channel: ChannelConfig {
                // Never reached: the source and the clock are handed in whole.
                metadata_base_url: "https://example.invalid".to_string(),
                artifact_base_url: release.host.url(""),
            },
            embedded_root: embedded_root.clone(),
            target: Some(LINUX.to_string()),
            check_interval: Duration::from_secs(3600),
            auto_apply,
            workers: WorkerConfig::default(),
        };
        let trust = TrustStore::new(&config.trust_state_dir);
        let channel = Channel::new(source, clock, embedded_root, trust);
        Updater::over(config, channel)
    }

    fn cache_dir(&self) -> PathBuf {
        self.dir.path().join("cache")
    }

    fn trust_state_dir(&self) -> PathBuf {
        self.dir.path().join("state")
    }

    fn cache(&self) -> Cache {
        Cache::new(self.cache_dir())
    }
}

/// Wait for the updater's thread to finish whatever it is doing.
fn settle(updater: &Updater) {
    settle_within(updater, SETTLE);
}

fn settle_within(updater: &Updater, within: Duration) {
    let deadline = Instant::now() + within;
    while updater.poll().phase != Phase::Idle {
        assert!(Instant::now() < deadline, "the updater never went idle");
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// Every generation the install root holds, in a comparable order.
fn installed_ids(updater: &Updater) -> Vec<String> {
    let mut ids: Vec<String> = updater
        .generations()
        .unwrap()
        .iter()
        .map(|generation| generation.id().to_string())
        .collect();
    ids.sort();
    ids
}

fn cached_at(client: &Client, artifact: &Payload) -> PathBuf {
    let digest = artifact.entry["sha256"].as_str().expect("a digest");
    client
        .cache()
        .path_for(&ArtifactDigest::from_hex(digest).expect("a digest"))
}

/// Install `release` from a channel that is already serving it, and report the
/// generation that landed.
fn install(client: &Client, channel: &FixtureChannel, release: &Release) -> String {
    let updater = client.updater(channel, release, true, now());
    assert!(updater.tick());
    settle(&updater);
    let generation = updater.next_completed().expect("a published generation");
    assert_eq!(generation.id(), release.generation_id());
    generation.id().to_string()
}

#[test]
fn a_published_release_set_is_checked_planned_applied_and_installed() {
    let release = Release::new(1, REVISION, &["spu", "uade"]);
    let mut channel = FixtureChannel::init(now());
    channel.publish(&release.manifest, now());

    let client = Client::new();
    let updater = client.updater(&channel, &release, false, now());

    assert!(updater.check_now(Priority::User));
    settle(&updater);

    let plan = updater
        .poll()
        .available
        .expect("the channel offers an update");
    let planned: Vec<&str> = plan
        .artifacts
        .iter()
        .map(|artifact| artifact.path.as_str())
        .collect();
    assert_eq!(planned, release.paths(), "the plan is what was published");
    assert_eq!(plan.total_bytes, release.total_bytes());
    assert_eq!(release.fetches(), 0, "a plan moves no artifact bytes");

    assert!(updater.apply(&plan, Priority::User));
    settle(&updater);

    let generation = updater.next_completed().expect("a published generation");
    assert_eq!(
        generation.id(),
        release.generation_id(),
        "a generation is named by the digest of the manifest that produced it"
    );
    assert_eq!(generation.artifacts().len(), release.payloads.len());
    for (installed, published) in generation.artifacts().iter().zip(&release.payloads) {
        assert_eq!(
            std::fs::read(generation.path_of(installed)).unwrap(),
            published.bytes,
            "{} did not arrive as it was published",
            installed.name
        );
    }
}

#[test]
fn a_second_check_against_an_unchanged_channel_is_up_to_date_and_fetches_nothing() {
    let release = Release::new(1, REVISION, &["spu"]);
    let mut channel = FixtureChannel::init(now());
    channel.publish(&release.manifest, now());

    let client = Client::new();
    let updater = client.updater(&channel, &release, true, now());
    assert!(updater.tick());
    settle(&updater);
    assert_eq!(
        updater.next_completed().expect("a generation").id(),
        release.generation_id()
    );

    let fetched = release.fetches();
    assert!(updater.check_now(Priority::Background));
    settle(&updater);

    let status = updater.poll();
    assert_eq!(
        status.last_check.expect("a recorded check").conclusion,
        Conclusion::UpToDate
    );
    assert_eq!(status.available, None);
    assert_eq!(
        release.fetches(),
        fetched,
        "an unchanged channel costs no bytes"
    );
}

#[test]
fn republishing_an_identical_set_resolves_to_the_same_generation_and_downloads_nothing() {
    let release = Release::new(1, REVISION, &["spu"]);
    let mut channel = FixtureChannel::init(now());
    channel.publish(&release.manifest, now());

    let client = Client::new();
    let first = install(&client, &channel, &release);
    let fetched = release.fetches();

    // The publisher republishes the same bytes: every online role advances, the
    // target does not.
    channel.publish(&release.manifest, now());

    // A restart, so the check offers the set rather than reporting up to date.
    // The apply is then what has to resolve it to the generation already there.
    let updater = client.updater(&channel, &release, true, now());
    assert!(updater.tick());
    settle(&updater);

    assert_eq!(
        updater.next_completed().expect("a generation").id(),
        first,
        "an identical set is the generation on disk, not another copy of it"
    );
    assert_eq!(installed_ids(&updater), [first]);
    assert_eq!(release.fetches(), fetched, "nothing was downloaded again");
}

#[test]
fn a_tampered_artifact_is_quarantined_and_the_installed_generation_survives() {
    let first = Release::new(1, REVISION, &["spu"]);
    let mut channel = FixtureChannel::init(now());
    channel.publish(&first.manifest, now());

    let client = Client::new();
    let installed = install(&client, &channel, &first);

    // The channel moves on, and the host serving the new set hands back bytes
    // the manifest never named.
    let forged = Release::tampered(2, NEXT_REVISION, &["spu"]);
    channel.publish(&forged.manifest, now());

    let updater = client.updater(&channel, &forged, true, now());
    assert!(updater.tick());
    settle(&updater);

    let status = updater.poll();
    assert!(
        status.last_error.is_some(),
        "the apply must report the refusal: {status:?}"
    );
    assert_eq!(updater.next_completed(), None);
    assert_eq!(
        installed_ids(&updater),
        [installed],
        "the generation already installed is untouched"
    );

    for artifact in &forged.payloads {
        let cached = cached_at(&client, artifact);
        assert!(
            !cached.exists(),
            "the bytes that failed are still cached at {}",
            cached.display()
        );
    }
}

#[test]
fn expired_channel_metadata_is_refused_until_a_resign_clears_it() {
    let release = Release::new(1, REVISION, &["spu"]);
    let mut channel = FixtureChannel::init(now());
    channel.publish(&release.manifest, now());

    // Timestamp metadata is signed for 90 days; a day past that nobody has
    // re-signed it.
    let late = days_on(91);
    let client = Client::new();
    {
        let updater = client.updater(&channel, &release, true, late);
        assert!(updater.tick());
        settle(&updater);

        let status = updater.poll();
        assert_eq!(
            status.last_check.expect("a recorded check").conclusion,
            Conclusion::Failed
        );
        assert_eq!(status.available, None);
        assert!(updater.generations().unwrap().is_empty());
        assert_eq!(release.fetches(), 0, "nothing expired is ever fetched");
    }

    // The scheduled re-sign is what clears it, without republishing anything.
    channel.resign(late);
    let updater = client.updater(&channel, &release, true, late);
    assert!(updater.tick());
    settle(&updater);
    assert_eq!(
        updater.next_completed().expect("a generation").id(),
        release.generation_id()
    );
}

#[test]
fn a_validly_signed_older_chain_is_refused_by_the_version_floor() {
    let first = Release::new(1, REVISION, &["spu"]);
    let mut channel = FixtureChannel::init(now());
    channel.publish(&first.manifest, now());

    let client = Client::new();
    let older = install(&client, &channel, &first);

    // A mirror that keeps serving this generation after the channel moves on.
    let stale = channel.frozen_copy();
    let second = Release::new(2, NEXT_REVISION, &["uade"]);
    channel.publish(&second.manifest, now());
    let newer = install(&client, &channel, &second);

    // The verified-metadata half is written best-effort by the TUF client — a
    // failed write is logged, not raised — so it can be absent on a check that
    // otherwise succeeded. The floor is what has to hold on its own.
    std::fs::remove_dir_all(client.trust_state_dir().join("metadata")).unwrap();

    let updater = client.updater_over(
        stale.source(),
        channel.root(),
        &first,
        true,
        Arc::new(At(now())),
    );
    assert!(updater.tick());
    settle(&updater);

    let status = updater.poll();
    assert_eq!(
        status.last_check.expect("a recorded check").conclusion,
        Conclusion::Failed
    );
    let refusal = status.last_error.expect("the refusal is reported");
    assert!(
        refusal.contains("below the trusted floor"),
        "the floor is what refused it: {refusal}"
    );

    let mut expected = vec![older, newer];
    expected.sort();
    assert_eq!(
        installed_ids(&updater),
        expected,
        "a refused rollback installs nothing and removes nothing"
    );
}

/// Opt-in: check and apply whatever the live channel is serving right now.
///
/// The one run where the publisher is the `retrovert-publish` CLI rather than
/// the fixture: the channel it signed is read here over real HTTPS, against the
/// host's own `Date` header, with the artifacts fetched from the release that
/// carries them.
///
/// ```text
/// RETROVERT_LIVE_CHANNEL_URL=https://…/dev/channel-metadata/ \
/// RETROVERT_LIVE_ARTIFACT_URL=https://…/dev/v1/ \
/// RETROVERT_LIVE_CHANNEL_ROOT=dev/root.json \
/// RETROVERT_LIVE_TARGET=linux-x86_64 \
///   cargo test round_trip -- --ignored
/// ```
///
/// `RETROVERT_LIVE_TARGET` is optional; without it the run takes only the
/// artifacts the manifest gives no target.
#[test]
#[ignore = "reaches the network and turns over with the channel's release schedule"]
fn the_live_channel_round_trips() {
    let (Ok(metadata_base_url), Ok(artifact_base_url), Ok(root_path)) = (
        std::env::var("RETROVERT_LIVE_CHANNEL_URL"),
        std::env::var("RETROVERT_LIVE_ARTIFACT_URL"),
        std::env::var("RETROVERT_LIVE_CHANNEL_ROOT"),
    ) else {
        panic!(
            "set RETROVERT_LIVE_CHANNEL_URL, RETROVERT_LIVE_ARTIFACT_URL and \
             RETROVERT_LIVE_CHANNEL_ROOT"
        );
    };

    let dir = TempDir::new().unwrap();
    let updater = Updater::new(UpdaterConfig {
        install_root: dir.path().join("install"),
        cache_dir: dir.path().join("cache"),
        trust_state_dir: dir.path().join("state"),
        channel: ChannelConfig {
            metadata_base_url,
            artifact_base_url,
        },
        embedded_root: std::fs::read(&root_path).expect("the embedded root"),
        target: std::env::var("RETROVERT_LIVE_TARGET").ok(),
        check_interval: Duration::from_secs(3600),
        auto_apply: true,
        workers: WorkerConfig::default(),
    })
    .expect("an https channel");

    assert!(updater.check_now(Priority::User));
    settle_within(&updater, LIVE_SETTLE);

    let status = updater.poll();
    let generation = updater
        .next_completed()
        .unwrap_or_else(|| panic!("the live channel installed nothing: {status:?}"));
    assert!(!generation.id().is_empty());

    assert!(updater.check_now(Priority::Background));
    settle_within(&updater, LIVE_SETTLE);
    assert_eq!(
        updater
            .poll()
            .last_check
            .expect("a recorded check")
            .conclusion,
        Conclusion::UpToDate
    );
}
