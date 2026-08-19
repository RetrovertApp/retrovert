//! Resolving a channel to an authenticated manifest, and refusing to.
//!
//! Everything here runs against a channel published into a temporary directory.

mod fixture_channel;
mod fixture_server;

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use fixture_channel::{FixtureChannel, KeySet, manifest_bytes};
use fixture_server::{Body, FixtureServer};
use jiff::tz::TimeZone;
use jiff::{Span, Timestamp};
use retrovert_tuf::manifest;
use retrovert_updater::channel::{
    Authenticated, Channel, Check, Clock, Error, HostDate, HttpSource, NetworkTime, TrustStore,
};
use retrovert_updater::transport::Transport;
use sigstore_tuf::Error as TufError;
use tempfile::TempDir;

/// A fixed instant, so every expiry assertion has an exact expected value.
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

/// A clock standing in for the network, so a test can date a check exactly.
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

fn channel_at(fixture: &FixtureChannel, trust: &Path, at: Timestamp) -> Channel {
    Channel::new(
        fixture.source(),
        Arc::new(At(at)),
        fixture.root(),
        TrustStore::new(trust),
    )
}

fn authenticated(check: Check) -> Authenticated {
    match check {
        Check::Authenticated(authenticated) => authenticated,
        Check::Skipped => panic!("the check was skipped"),
    }
}

#[test]
fn a_published_channel_resolves_to_its_manifest() {
    let trust = TempDir::new().unwrap();
    let mut fixture = FixtureChannel::init(now());
    let bytes = manifest_bytes(7, "abc1234");
    let generation_id = fixture.publish(&bytes, now());

    let check = channel_at(&fixture, trust.path(), now())
        .authenticate()
        .unwrap();

    let resolved = authenticated(check);
    assert_eq!(resolved.generation_id, generation_id);
    assert_eq!(resolved.verified_at, now());
    assert_eq!(resolved.manifest.version, 7);
    assert_eq!(resolved.manifest.source_revision, "abc1234");
    assert_eq!(resolved.manifest.artifacts.len(), 1);
    assert_eq!(resolved.manifest.artifacts[0].name, "spu");
}

#[test]
fn a_republished_channel_resolves_to_the_newer_generation() {
    let trust = TempDir::new().unwrap();
    let mut fixture = FixtureChannel::init(now());
    fixture.publish(&manifest_bytes(1, "aaa1111"), now());
    let first = authenticated(
        channel_at(&fixture, trust.path(), now())
            .authenticate()
            .unwrap(),
    );

    let second_id = fixture.publish(&manifest_bytes(2, "bbb2222"), now());
    let second = authenticated(
        channel_at(&fixture, trust.path(), now())
            .authenticate()
            .unwrap(),
    );

    assert_ne!(first.generation_id, second.generation_id);
    assert_eq!(second.generation_id, second_id);
    assert_eq!(second.manifest.source_revision, "bbb2222");
}

#[test]
fn a_check_without_network_time_is_skipped_rather_than_run_on_the_local_clock() {
    let trust = TempDir::new().unwrap();
    let mut fixture = FixtureChannel::init(now());
    fixture.publish(&manifest_bytes(1, "abc1234"), now());

    let store = TrustStore::new(trust.path().join("state"));
    let mut channel = Channel::new(
        fixture.source(),
        Arc::new(Unreachable),
        fixture.root(),
        store.clone(),
    );

    assert!(matches!(channel.authenticate().unwrap(), Check::Skipped));
    assert!(
        !store.dir().exists(),
        "a skipped check must leave trust state untouched"
    );
}

#[test]
fn trust_state_persists_across_restarts_and_carries_a_version_floor() {
    let trust = TempDir::new().unwrap();
    let mut fixture = FixtureChannel::init(now());
    fixture.publish(&manifest_bytes(1, "abc1234"), now());

    let store = TrustStore::new(trust.path().join("state"));
    assert_eq!(store.floor().unwrap().timestamp, 0);

    channel_at(&fixture, store.dir(), now())
        .authenticate()
        .unwrap();

    // A publish advances every online role past the version `init` wrote.
    let floor = store.floor().unwrap();
    assert_eq!(floor.root, 1);
    assert_eq!(floor.timestamp, 2);
    assert_eq!(floor.snapshot, 2);
    assert_eq!(floor.targets, 2);

    // A restart is a new process reading the same directory.
    let restarted = TrustStore::new(store.dir());
    assert_eq!(restarted.floor().unwrap(), floor);
    assert!(
        store.dir().join("metadata").join("timestamp.json").exists(),
        "the newest verified metadata is trust state too"
    );
}

#[test]
fn tampered_manifest_bytes_are_refused() {
    let trust = TempDir::new().unwrap();
    let mut fixture = FixtureChannel::init(now());
    let bytes = manifest_bytes(1, "abc1234");
    let generation_id = fixture.publish(&bytes, now());

    // Same length, different content: the pinned length still matches, so the
    // refusal has to come from the digest.
    let mut forged = bytes.clone();
    let last = forged.len() - 2;
    forged[last] = if forged[last] == b' ' { b'\t' } else { b' ' };
    assert_eq!(forged.len(), bytes.len());

    let mut channel = Channel::new(
        fixture.tampered(&format!("{generation_id}.manifest.json"), forged),
        Arc::new(At(now())),
        fixture.root(),
        TrustStore::new(trust.path()),
    );

    let err = channel.authenticate().unwrap_err();
    assert!(
        matches!(err, Error::Tuf(TufError::IntegrityMismatch(ref what))
            if what.contains(manifest::TARGET_PATH)),
        "the manifest target is what failed to match: {err}"
    );
}

#[test]
fn expired_metadata_is_refused() {
    let trust = TempDir::new().unwrap();
    let mut fixture = FixtureChannel::init(now());
    fixture.publish(&manifest_bytes(1, "abc1234"), now());

    // Timestamp metadata is signed for 14 days; a fortnight later nobody has
    // re-signed it.
    let late = days_on(15);
    let err = channel_at(&fixture, trust.path(), late)
        .authenticate()
        .unwrap_err();

    assert!(
        matches!(err, Error::Tuf(TufError::Expired { ref role, .. }) if role == "timestamp"),
        "{err}"
    );

    // The scheduled re-sign is what clears it, without republishing anything.
    fixture.resign(late);
    let resolved = authenticated(
        channel_at(&fixture, trust.path(), late)
            .authenticate()
            .unwrap(),
    );
    assert_eq!(resolved.manifest.source_revision, "abc1234");
}

#[test]
fn metadata_not_signed_by_the_embedded_root_is_refused() {
    let trust = TempDir::new().unwrap();
    let mut ours = FixtureChannel::init(now());
    ours.publish(&manifest_bytes(1, "abc1234"), now());

    let mut theirs = FixtureChannel::init_with(KeySet::seeded(50), now());
    theirs.publish(&manifest_bytes(9, "def5678"), now());

    let mut channel = Channel::new(
        theirs.source(),
        Arc::new(At(now())),
        ours.root(),
        TrustStore::new(trust.path()),
    );

    let err = channel.authenticate().unwrap_err();
    assert!(
        matches!(err, Error::Tuf(TufError::ThresholdNotMet { .. })),
        "{err}"
    );
}

#[test]
fn an_older_but_validly_signed_chain_is_refused() {
    let trust = TempDir::new().unwrap();
    let mut fixture = FixtureChannel::init(now());
    fixture.publish(&manifest_bytes(1, "aaa1111"), now());
    channel_at(&fixture, trust.path(), now())
        .authenticate()
        .unwrap();

    // A host that keeps serving this generation after the channel moves on.
    let frozen = fixture.frozen_copy();
    fixture.publish(&manifest_bytes(2, "bbb2222"), now());
    channel_at(&fixture, trust.path(), now())
        .authenticate()
        .unwrap();

    let mut stale = Channel::new(
        frozen.source(),
        Arc::new(At(now())),
        fixture.root(),
        TrustStore::new(trust.path()),
    );

    // With the verified metadata still on disk the TUF client itself holds the
    // line, before the floor is ever consulted.
    let err = stale.authenticate().unwrap_err();
    assert!(
        matches!(err, Error::Tuf(TufError::Rollback { .. })),
        "{err}"
    );
}

#[test]
fn the_floor_still_refuses_a_rollback_once_the_verified_metadata_is_gone() {
    let trust = TempDir::new().unwrap();
    let store = TrustStore::new(trust.path());
    let mut fixture = FixtureChannel::init(now());
    fixture.publish(&manifest_bytes(1, "aaa1111"), now());
    channel_at(&fixture, store.dir(), now())
        .authenticate()
        .unwrap();

    let frozen = fixture.frozen_copy();
    fixture.publish(&manifest_bytes(2, "bbb2222"), now());
    channel_at(&fixture, store.dir(), now())
        .authenticate()
        .unwrap();

    // The metadata half is written best-effort by the TUF client — a failed
    // write is logged, not raised — so it can be absent on a check that
    // otherwise succeeded. The floor is what still refuses the rollback.
    std::fs::remove_dir_all(store.dir().join("metadata")).unwrap();

    let mut stale = Channel::new(
        frozen.source(),
        Arc::new(At(now())),
        fixture.root(),
        TrustStore::new(store.dir()),
    );

    let err = stale.authenticate().unwrap_err();
    assert!(
        matches!(
            err,
            Error::Rollback {
                role: "timestamp",
                floor: 3,
                offered: 2
            }
        ),
        "{err}"
    );
}

#[test]
fn bytes_that_authenticate_but_are_not_a_readable_manifest_are_refused() {
    let trust = TempDir::new().unwrap();
    let store = TrustStore::new(trust.path());
    let mut fixture = FixtureChannel::init(now());
    fixture.publish(br#"{"schema":2}"#, now());

    let err = channel_at(&fixture, store.dir(), now())
        .authenticate()
        .unwrap_err();
    assert!(matches!(err, Error::Manifest(_)), "{err}");

    // The signatures over that target were sound, so the chain carrying it is
    // still the newest one seen and the floor moved with it.
    assert_eq!(store.floor().unwrap().timestamp, 2);
}

#[test]
fn a_wiped_trust_directory_returns_the_client_to_the_embedded_root() {
    let trust = TempDir::new().unwrap();
    let store = TrustStore::new(trust.path().join("state"));
    let mut fixture = FixtureChannel::init(now());
    fixture.publish(&manifest_bytes(1, "aaa1111"), now());
    let frozen = fixture.frozen_copy();
    fixture.publish(&manifest_bytes(2, "bbb2222"), now());
    channel_at(&fixture, store.dir(), now())
        .authenticate()
        .unwrap();

    // A factory reset takes the trust directory with it, floor included. What
    // is left is the root in the firmware, which the older chain does verify
    // against.
    std::fs::remove_dir_all(store.dir()).unwrap();
    let mut reset = Channel::new(
        frozen.source(),
        Arc::new(At(now())),
        fixture.root(),
        TrustStore::new(store.dir()),
    );
    let resolved = authenticated(reset.authenticate().unwrap());
    assert_eq!(resolved.manifest.source_revision, "aaa1111");

    // So a reset cannot smuggle an old release in: one check against the live
    // channel walks the device back to current.
    let resolved = authenticated(
        channel_at(&fixture, store.dir(), now())
            .authenticate()
            .unwrap(),
    );
    assert_eq!(resolved.manifest.source_revision, "bbb2222");
}

#[test]
fn a_channel_authenticates_over_http() {
    let trust = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let mut fixture = FixtureChannel::init(now());
    let generation_id = fixture.publish(&manifest_bytes(1, "abc1234"), now());

    let routes: HashMap<String, Body> = fixture
        .assets()
        .into_iter()
        .map(|(name, bytes)| (format!("/{name}"), Body::instant(bytes, "\"fixed\"")))
        .collect();
    let server = FixtureServer::start(routes);

    let transport = Arc::new(Transport::new(cache.path()));
    let mut channel = Channel::new(
        Arc::new(HttpSource::new(Arc::clone(&transport), &server.url(""))),
        Arc::new(At(now())),
        fixture.root(),
        TrustStore::new(trust.path()),
    );

    let resolved = authenticated(channel.authenticate().unwrap());
    assert_eq!(resolved.generation_id, generation_id);

    let polled = server.records();
    assert!(
        polled
            .iter()
            .any(|record| record.path.starts_with("/timestamp.json?t=")),
        "the mutable entry point must be cache-busted: {polled:?}"
    );
    assert!(
        polled
            .iter()
            .any(|record| record.path == format!("/{generation_id}.manifest.json")),
        "the target is fetched under its consistent-snapshot name: {polled:?}"
    );
}

#[test]
fn a_plaintext_channel_is_refused_before_a_request_is_made() {
    let cache = TempDir::new().unwrap();
    let trust = TempDir::new().unwrap();
    let transport = Arc::new(Transport::new(cache.path()));

    let err = Channel::https(
        &transport,
        "http://host/dev/channel-metadata",
        Vec::new(),
        TrustStore::new(trust.path()),
    )
    .unwrap_err();
    assert!(matches!(err, Error::Insecure(_)), "{err}");
}

/// Opt-in: authenticate whatever the live channel is serving right now.
///
/// ```text
/// RETROVERT_LIVE_CHANNEL_URL=https://… RETROVERT_LIVE_CHANNEL_ROOT=root.json \
///   cargo test --test channel_authentication -- --ignored
/// ```
#[test]
#[ignore = "reaches the network and turns over with the channel's re-sign schedule"]
fn the_live_channel_authenticates() {
    let (Ok(base_url), Ok(root_path)) = (
        std::env::var("RETROVERT_LIVE_CHANNEL_URL"),
        std::env::var("RETROVERT_LIVE_CHANNEL_ROOT"),
    ) else {
        panic!("set RETROVERT_LIVE_CHANNEL_URL and RETROVERT_LIVE_CHANNEL_ROOT");
    };

    let cache = TempDir::new().unwrap();
    let trust = TempDir::new().unwrap();
    let transport = Arc::new(Transport::new(cache.path()));
    let root = std::fs::read(&root_path).expect("the embedded root");

    // The real clock too, not a fixture one: whether the host will tell us the
    // time is part of what this smoke test is checking.
    let time = HostDate::new(Arc::clone(&transport), &base_url).unwrap();
    assert!(time.network_time().is_some(), "{base_url} served no Date");

    let mut channel = Channel::https(&transport, &base_url, root, TrustStore::new(trust.path()))
        .expect("an https channel");
    let resolved = authenticated(channel.authenticate().expect("the live channel"));
    assert!(!resolved.generation_id.is_empty());
}
