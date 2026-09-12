//! The transfer queue against a real HTTP server.
//!
//! The server is in-process and hermetic (see [`fixture_server`]), so these run
//! wherever `cargo test` runs rather than skipping when a LAN host is missing.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use crate::queue::{Config, EntryId, Failure, Priority, QUEUE_MAX, Request, State, TransferQueue};
use crate::testing::fixture_server::{Body, FixtureServer};
use crate::transport::{ArtifactDigest, Transport};
use sha2::{Digest, Sha256};

/// Small enough to arrive in one piece.
const SMALL_SIZE: usize = 64 * 1024;
/// Served in pieces slowly enough that a test can catch it mid-transfer.
const LARGE_SIZE: usize = 1024 * 1024;
const LARGE_PIECE: usize = 16 * 1024;
const LARGE_DELAY: Duration = Duration::from_millis(40);
/// Served in pieces small enough that a slow body outlasts several large ones,
/// so a preemption test cannot have its background work finish under it while
/// a loaded runner keeps the test thread off the CPU.
const SLOW_PIECE: usize = 4 * 1024;

const SMALL: &str = "/small";
const LARGE_A: &str = "/large-a";
const LARGE_B: &str = "/large-b";
const LARGE_C: &str = "/large-c";
const LARGE_D: &str = "/large-d";
const SLOW_A: &str = "/slow-a";
const SLOW_B: &str = "/slow-b";
const STALLED: &str = "/stalled";

/// How long the stalled route sits on its response head. Long enough that a
/// test reliably gets a request in while the queue holds no transfer handle,
/// short enough not to dominate the suite.
const HEADER_STALL: Duration = Duration::from_secs(1);

/// Long enough to cover a whole throttled body several times over on a loaded
/// CI runner.
const TERMINAL_TIMEOUT: Duration = Duration::from_secs(60);
const POLL: Duration = Duration::from_millis(20);

/// Progress a yielding transfer may still make without having kept its worker.
/// It drains the piece already in flight before its pause lands, which on a
/// slow body is well under a hundredth of it; a transfer that really held its
/// worker covers a quarter of its body over the same interval.
const YIELD_SLACK: f32 = 0.05;

/// Deterministic bytes that differ per route, so every route has its own
/// digest.
fn body_bytes(len: usize, seed: u64) -> Vec<u8> {
    (0..len_of_usize(len))
        .map(|i| {
            i.wrapping_mul(2_654_435_761)
                .wrapping_add(seed)
                .to_le_bytes()[0]
        })
        .collect()
}

fn len_of(bytes: &[u8]) -> u64 {
    len_of_usize(bytes.len())
}

fn len_of_usize(len: usize) -> u64 {
    u64::try_from(len).expect("a fixture body fits in a u64")
}

fn digest_of(bytes: &[u8]) -> ArtifactDigest {
    ArtifactDigest::from_hex(&hex::encode(Sha256::digest(bytes))).expect("a 64-character digest")
}

struct Fixture {
    // Declaration order is drop order: the queue blocks on its workers, so it
    // must go before the server they are talking to and the directory they are
    // writing into.
    queue: TransferQueue,
    server: FixtureServer,
    _dir: tempfile::TempDir,
    small: Vec<u8>,
    large_a: Vec<u8>,
    large_b: Vec<u8>,
    large_c: Vec<u8>,
    large_d: Vec<u8>,
    slow_a: Vec<u8>,
    slow_b: Vec<u8>,
    stalled: Vec<u8>,
}

impl Fixture {
    fn new() -> Self {
        Self::with_config(Config::default())
    }

    fn with_workers(workers: usize) -> Self {
        Self::with_config(Config {
            workers,
            ..Config::default()
        })
    }

    fn with_config(config: Config) -> Self {
        let small = body_bytes(SMALL_SIZE, 1);
        let large_a = body_bytes(LARGE_SIZE, 2);
        let large_b = body_bytes(LARGE_SIZE, 3);
        let large_c = body_bytes(LARGE_SIZE, 4);
        let large_d = body_bytes(LARGE_SIZE, 5);
        let slow_a = body_bytes(LARGE_SIZE, 6);
        let slow_b = body_bytes(LARGE_SIZE, 7);
        let stalled = body_bytes(LARGE_SIZE, 8);

        let mut routes = HashMap::new();
        routes.insert(SMALL.to_string(), Body::instant(small.clone(), "\"small\""));
        routes.insert(
            STALLED.to_string(),
            Body::throttled(stalled.clone(), "\"stalled\"", LARGE_PIECE, LARGE_DELAY)
                .stalling(HEADER_STALL),
        );
        for (path, bytes, etag, piece) in [
            (LARGE_A, &large_a, "\"large-a\"", LARGE_PIECE),
            (LARGE_B, &large_b, "\"large-b\"", LARGE_PIECE),
            (LARGE_C, &large_c, "\"large-c\"", LARGE_PIECE),
            (LARGE_D, &large_d, "\"large-d\"", LARGE_PIECE),
            (SLOW_A, &slow_a, "\"slow-a\"", SLOW_PIECE),
            (SLOW_B, &slow_b, "\"slow-b\"", SLOW_PIECE),
        ] {
            routes.insert(
                path.to_string(),
                Body::throttled(bytes.clone(), etag, piece, LARGE_DELAY),
            );
        }

        let dir = tempfile::tempdir().expect("a scratch cache directory");
        let server = FixtureServer::start(routes);
        Self {
            queue: TransferQueue::with_config(Transport::new(dir.path()), config),
            server,
            _dir: dir,
            small,
            large_a,
            large_b,
            large_c,
            large_d,
            slow_a,
            slow_b,
            stalled,
        }
    }

    fn bytes(&self, path: &str) -> &[u8] {
        match path {
            SMALL => &self.small,
            LARGE_A => &self.large_a,
            LARGE_B => &self.large_b,
            LARGE_C => &self.large_c,
            LARGE_D => &self.large_d,
            SLOW_A => &self.slow_a,
            SLOW_B => &self.slow_b,
            STALLED => &self.stalled,
            other => panic!("no fixture body for {other}"),
        }
    }

    fn request(&self, path: &str, priority: Priority) -> Request {
        Request {
            url: self.server.url(path),
            digest: digest_of(self.bytes(path)),
            expected_size: None,
            priority,
        }
    }

    fn queue(&self, request: Request) -> EntryId {
        self.queue.queue(request).expect("a free queue slot")
    }

    fn wait_state(&self, id: EntryId, state: State) -> bool {
        wait_for(TERMINAL_TIMEOUT, || self.queue.state(id) == state)
    }

    /// How many of `ids` are in flight right now.
    fn downloading(&self, ids: &[EntryId]) -> usize {
        ids.iter()
            .filter(|&&id| self.queue.state(id) == State::Downloading)
            .count()
    }

    /// Each entry's progress. Progress only ever climbs, so comparing two of
    /// these tells a test which transfers ran over an interval however
    /// irregularly it was scheduled in between.
    fn progress(&self, ids: &[EntryId]) -> Vec<f32> {
        ids.iter().map(|&id| self.queue.progress(id)).collect()
    }

    /// Read back a completed transfer's cached bytes.
    fn cached(&self, id: EntryId) -> Vec<u8> {
        let path = self.queue.path(id).expect("a cached path");
        std::fs::read(&path).expect("the cached file")
    }

    /// Spin until `id` settles, and report the state it settled in.
    fn wait_terminal(&self, id: EntryId) -> State {
        wait_for(TERMINAL_TIMEOUT, || {
            matches!(
                self.queue.state(id),
                State::Complete | State::Failed | State::Cancelled
            )
        });
        self.queue.state(id)
    }
}

/// Spin until `predicate` holds or the timeout elapses, and report whether it
/// held.
fn wait_for(timeout: Duration, mut predicate: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if predicate() {
            return true;
        }
        std::thread::sleep(POLL);
    }
    predicate()
}

#[test]
fn a_transfer_completes_into_the_cache() {
    let fixture = Fixture::new();
    let id = fixture.queue(fixture.request(SMALL, Priority::User));

    assert_eq!(fixture.wait_terminal(id), State::Complete);
    assert_eq!(fixture.queue.failure(id), None);

    let path = fixture
        .queue
        .path(id)
        .expect("a completed transfer has a path");
    assert_eq!(
        std::fs::read(&path).expect("the cached file"),
        fixture.small
    );
    assert!(
        fixture
            .queue
            .transport()
            .cache()
            .is_complete(&digest_of(&fixture.small))
    );
    assert!((fixture.queue.progress(id) - 1.0).abs() < 1e-6);
}

#[test]
fn a_complete_cache_entry_is_served_without_a_request() {
    let fixture = Fixture::new();
    let first = fixture.queue(fixture.request(SMALL, Priority::User));
    assert_eq!(fixture.wait_terminal(first), State::Complete);

    let requests = fixture.server.request_count(SMALL);
    assert!(requests > 0, "the first transfer must have gone out");

    let second = fixture.queue(fixture.request(SMALL, Priority::User));
    assert_eq!(fixture.wait_terminal(second), State::Complete);
    assert_eq!(
        fixture.server.request_count(SMALL),
        requests,
        "a complete cache entry must not be re-fetched"
    );
    // The cached entry reports as fully transferred, not as no progress at all.
    assert!((fixture.queue.progress(second) - 1.0).abs() < 1e-6);
}

/// A server free to stream forever must not be read to its end: once the body
/// runs past the length the request named, the transfer is stopped where it
/// stands and its bytes are discarded.
#[test]
fn a_body_running_past_the_named_length_is_stopped_mid_flight() {
    let fixture = Fixture::new();
    let expected = 4 * 1024_u64;
    let mut request = fixture.request(LARGE_A, Priority::User);
    request.expected_size = Some(expected);
    let id = fixture.queue(request);

    assert_eq!(fixture.wait_terminal(id), State::Failed);
    let failure = fixture.queue.failure(id).expect("a recorded failure");
    assert!(
        matches!(failure, Failure::Oversize { expected: named, .. } if named == expected),
        "{failure}"
    );

    // Nothing poisoned is left behind and nothing resumes onto refused bytes.
    let cache = fixture.queue.transport().cache();
    let digest = digest_of(&fixture.large_a);
    assert!(!cache.is_complete(&digest));
    assert!(!cache.has_partial(&digest));
    assert!(!cache.path_for(&digest).exists());
}

#[test]
fn a_user_transfer_preempts_a_background_one() {
    let fixture = Fixture::new();
    let background = fixture.queue(fixture.request(LARGE_A, Priority::Background));
    assert!(
        fixture.wait_state(background, State::Downloading),
        "the background transfer must be in flight for the preemption to mean anything"
    );

    let user = fixture.queue(fixture.request(LARGE_B, Priority::User));
    assert_eq!(fixture.wait_terminal(user), State::Complete);

    // The background transfer gave up the worker: it cannot have finished while
    // the user transfer held it.
    assert_ne!(fixture.queue.state(background), State::Complete);

    // ...and it picks itself back up once the user transfer is done.
    assert_eq!(fixture.wait_terminal(background), State::Complete);
    let path = fixture.queue.path(background).expect("a cached path");
    assert_eq!(
        std::fs::read(&path).expect("the cached file"),
        fixture.large_a
    );
}

/// Concurrency is the configured worker count, so two workers put two
/// transfers in flight at the same instant.
#[test]
fn two_workers_transfer_two_artifacts_at_once() {
    let fixture = Fixture::with_workers(2);
    assert_eq!(fixture.queue.workers(), 2);

    let a = fixture.queue(fixture.request(LARGE_A, Priority::Background));
    let b = fixture.queue(fixture.request(LARGE_B, Priority::Background));
    assert!(
        wait_for(TERMINAL_TIMEOUT, || fixture.downloading(&[a, b]) == 2),
        "both transfers must be in flight at once"
    );

    assert_eq!(fixture.wait_terminal(a), State::Complete);
    assert_eq!(fixture.wait_terminal(b), State::Complete);
    assert_eq!(fixture.cached(a), fixture.large_a);
    assert_eq!(fixture.cached(b), fixture.large_b);
}

/// The default configuration is one worker, and one worker never runs two
/// transfers at once however deep the queue is.
#[test]
fn one_worker_transfers_one_artifact_at_a_time() {
    let fixture = Fixture::new();
    assert_eq!(fixture.queue.workers(), 1);

    let a = fixture.queue(fixture.request(LARGE_A, Priority::Background));
    let b = fixture.queue(fixture.request(LARGE_B, Priority::Background));
    assert!(wait_for(TERMINAL_TIMEOUT, || fixture.downloading(&[a, b]) == 1));

    let both_complete = wait_for(TERMINAL_TIMEOUT, || {
        assert!(
            fixture.downloading(&[a, b]) <= 1,
            "one worker must not run both transfers"
        );
        fixture.queue.state(a) == State::Complete && fixture.queue.state(b) == State::Complete
    });
    assert!(both_complete, "both transfers run, one after the other");
}

/// Preemption is decided against the queue: with every worker busy on
/// background work, a queued user transfer displaces one of them, and the rest
/// keep their workers.
///
/// The background work is served slowly enough that it cannot finish under the
/// test, and the verdict is read off progress rather than off states caught at
/// an instant, so a runner that leaves this thread off the CPU for a while
/// cannot change the answer.
#[test]
fn a_user_transfer_preempts_one_of_several_background_ones() {
    let fixture = Fixture::with_workers(2);
    let a = fixture.queue(fixture.request(SLOW_A, Priority::Background));
    let b = fixture.queue(fixture.request(SLOW_B, Priority::Background));
    assert!(
        wait_for(TERMINAL_TIMEOUT, || fixture.downloading(&[a, b]) == 2),
        "both workers must be busy for the preemption to have a choice to make"
    );

    let user = fixture.queue(fixture.request(LARGE_C, Priority::User));
    assert!(
        fixture.wait_state(user, State::Downloading),
        "the user transfer takes the worker a background one gave up"
    );

    // Sample only while the user transfer still holds its worker: the yielded
    // transfer resumes the moment it settles, and would look like it had never
    // stopped.
    let before = fixture.progress(&[a, b]);
    let mut during = before.clone();
    while fixture.queue.state(user) == State::Downloading {
        during = fixture.progress(&[a, b]);
        std::thread::sleep(POLL);
    }
    assert_eq!(fixture.wait_terminal(user), State::Complete);

    // One background transfer held its worker across the whole user transfer
    // and covered a good quarter of its body; the other yielded and moved by
    // no more than the piece already in flight.
    let ran: Vec<bool> = (0..2)
        .map(|i| during[i] - before[i] > YIELD_SLACK)
        .collect();
    assert_eq!(
        ran.iter().filter(|&&r| r).count(),
        1,
        "one background transfer yields and one carries on, from {before:?} to {during:?}"
    );

    // The yielded one is not abandoned: it picks itself back up once the user
    // work drains.
    let yielded = if ran[0] { b } else { a };
    assert!(
        fixture.wait_state(yielded, State::Downloading),
        "the preempted transfer resumes"
    );
}

/// A preempted transfer waits for the queue's user work to drain, not for the
/// one transfer that displaced it: with several workers, another may still be
/// on user work when the first settles.
#[test]
fn preempted_transfers_resume_only_once_all_user_work_drains() {
    let fixture = Fixture::with_workers(2);
    let a = fixture.queue(fixture.request(SLOW_A, Priority::Background));
    let b = fixture.queue(fixture.request(SLOW_B, Priority::Background));
    assert!(wait_for(TERMINAL_TIMEOUT, || fixture.downloading(&[a, b]) == 2));

    let first = fixture.queue(fixture.request(LARGE_C, Priority::User));
    let second = fixture.queue(fixture.request(LARGE_D, Priority::User));
    assert!(
        wait_for(TERMINAL_TIMEOUT, || fixture.downloading(&[first, second])
            == 2),
        "both background transfers give up their workers to the user ones"
    );

    // Neither background transfer may advance while either user transfer is
    // still going. Compared against one baseline rather than the previous
    // sample, so a resume cannot slip between two polls unseen.
    let idled = fixture.progress(&[a, b]);
    let user_drained = wait_for(TERMINAL_TIMEOUT, || {
        let outstanding = fixture.downloading(&[first, second]) > 0;
        let moved = fixture.progress(&[a, b]);
        assert!(
            !outstanding || (0..2).all(|i| moved[i] - idled[i] <= YIELD_SLACK),
            "a preempted transfer resumed while user work was still in flight"
        );
        !outstanding
    });
    assert!(user_drained);

    assert_eq!(fixture.wait_terminal(first), State::Complete);
    assert_eq!(fixture.wait_terminal(second), State::Complete);

    // Both resume together once the last user transfer is out of the way.
    assert!(
        wait_for(TERMINAL_TIMEOUT, || fixture.downloading(&[a, b]) == 2),
        "both preempted transfers resume, not just the one whose worker came free: {:?} {:?}",
        fixture.queue.state(a),
        fixture.queue.state(b)
    );
}

#[test]
fn pausing_preserves_progress_and_resuming_completes() {
    let fixture = Fixture::new();
    let id = fixture.queue(fixture.request(LARGE_A, Priority::User));

    // Catch it mid-transfer: pause and resume only mean something in flight.
    assert!(wait_for(TERMINAL_TIMEOUT, || {
        fixture.queue.state(id) == State::Downloading && fixture.queue.progress(id) > 0.05
    }));

    assert!(fixture.queue.pause(id));
    assert!(fixture.wait_state(id, State::Paused));
    assert!(fixture.queue.progress(id) > 0.0);

    assert!(fixture.queue.resume(id));
    assert_eq!(fixture.wait_terminal(id), State::Complete);
    let path = fixture.queue.path(id).expect("a cached path");
    assert_eq!(
        std::fs::read(&path).expect("the cached file"),
        fixture.large_a
    );
}

/// A slot reads Downloading from the moment a worker claims it, but the queue
/// has nothing to ask until the transport has a response in hand. A request
/// landing in that window is carried across it by the slot's flag rather than
/// dropped against the missing handle — without which a transfer told to stop
/// runs on to Complete.
#[test]
fn a_pause_that_lands_before_the_transfer_starts_is_not_lost() {
    let fixture = Fixture::new();
    let id = fixture.queue(fixture.request(STALLED, Priority::User));

    // The claim publishes Downloading; the route then sits on its response
    // head, so what follows has no transfer to act on.
    assert!(fixture.wait_state(id, State::Downloading));
    assert!(
        fixture.queue.progress(id) <= 0.0,
        "the route must still be stalling for this to test the window"
    );
    assert!(fixture.queue.pause(id));

    assert!(
        fixture.wait_state(id, State::Paused),
        "the pause was dropped: the transfer settled {:?}",
        fixture.queue.state(id)
    );

    assert!(fixture.queue.resume(id));
    assert_eq!(fixture.wait_terminal(id), State::Complete);
    assert_eq!(fixture.cached(id), fixture.stalled);
}

/// Nothing yields while a worker is free. The rule is decided from the live
/// per-worker records rather than a flag a test hands in, so it is worth
/// driving end to end: one worker busy, one idle, and a user request that
/// takes the idle one without disturbing the background transfer.
#[test]
fn a_free_worker_takes_new_work_without_preempting_anything() {
    let fixture = Fixture::with_workers(2);
    let background = fixture.queue(fixture.request(SLOW_A, Priority::Background));
    assert!(fixture.wait_state(background, State::Downloading));

    // One worker is busy and the other is not, so this must not displace
    // anything.
    let before = fixture.progress(&[background]);
    let user = fixture.queue(fixture.request(LARGE_C, Priority::User));
    assert!(fixture.wait_state(user, State::Downloading));

    let mut during = before.clone();
    while fixture.queue.state(user) == State::Downloading {
        assert_ne!(
            fixture.queue.state(background),
            State::Preempted,
            "a free worker was available, so nothing should have yielded"
        );
        during = fixture.progress(&[background]);
        std::thread::sleep(POLL);
    }
    assert_eq!(fixture.wait_terminal(user), State::Complete);

    // It kept its worker throughout rather than merely avoiding the Preempted
    // state.
    assert!(
        during[0] - before[0] > YIELD_SLACK,
        "the background transfer ran alongside the user one, from {before:?} to {during:?}"
    );
}

/// A cancel is a durable promise, including against an entry that has been
/// marked to yield but whose worker has not physically paused it yet. The
/// stalled route holds that window open.
#[test]
fn a_cancel_of_a_preempted_transfer_is_honoured() {
    let fixture = Fixture::new();
    let background = fixture.queue(fixture.request(STALLED, Priority::Background));
    assert!(fixture.wait_state(background, State::Downloading));

    // The single worker is busy, so this marks the background entry Preempted
    // while that worker still owns the slot.
    let user = fixture.queue(fixture.request(SMALL, Priority::User));
    assert!(fixture.wait_state(background, State::Preempted));

    assert!(fixture.queue.cancel(background));

    // The verdict has to be read after the worker settles the slot, not off
    // the state `cancel` itself published: the settle is what can overwrite a
    // reported Cancelled with the preemption's Paused. With one worker, the
    // user transfer cannot start until that settle has happened, so its
    // completion is the signal that the window has closed.
    assert_eq!(fixture.wait_terminal(user), State::Complete);
    assert_eq!(
        fixture.queue.state(background),
        State::Cancelled,
        "the cancel was reported as applied, so the settle must not undo it"
    );
}

/// A preempted entry is still owned by its worker. Freeing the slot would let
/// the next `queue` hand it out from under the transfer still running in it.
#[test]
fn a_preempted_entry_cannot_be_removed_from_under_its_worker() {
    let fixture = Fixture::new();
    let background = fixture.queue(fixture.request(STALLED, Priority::Background));
    assert!(fixture.wait_state(background, State::Downloading));

    let user = fixture.queue(fixture.request(SMALL, Priority::User));
    assert!(fixture.wait_state(background, State::Preempted));

    assert!(
        !fixture.queue.remove(background),
        "the slot is still claimed, so it must not be freed"
    );
    assert_eq!(fixture.wait_terminal(user), State::Complete);

    // Released once the user work drains and the entry runs to completion.
    assert_eq!(fixture.wait_terminal(background), State::Complete);
    assert!(fixture.queue.remove(background));
}

#[test]
fn a_paused_transfer_resumes_from_its_partial_file() {
    let fixture = Fixture::new();
    let digest = digest_of(&fixture.large_a);
    let cache = fixture.queue.transport().cache().clone();

    let first = fixture.queue(fixture.request(LARGE_A, Priority::User));
    assert!(wait_for(TERMINAL_TIMEOUT, || {
        fixture.queue.state(first) == State::Downloading && fixture.queue.progress(first) > 0.10
    }));

    assert!(fixture.queue.pause(first));
    assert!(fixture.wait_state(first, State::Paused));

    // The sidecar records what is on disk, and the entry is partial rather than
    // complete.
    assert!(cache.has_partial(&digest));
    assert!(!cache.is_complete(&digest));
    let partial = std::fs::metadata(cache.path_for(&digest))
        .expect("a partial file")
        .len();
    assert!(partial > 0);

    // Cancelling a paused entry frees the slot without discarding what is on
    // disk.
    assert!(fixture.queue.cancel(first));
    assert!(fixture.wait_state(first, State::Cancelled));
    assert!(fixture.queue.remove(first));

    let second = fixture.queue(fixture.request(LARGE_A, Priority::User));
    assert_eq!(fixture.wait_terminal(second), State::Complete);

    // The second transfer spliced onto the partial file rather than starting
    // over: it asked for a range and the server answered 206.
    let resumed = fixture
        .server
        .records()
        .into_iter()
        .find(|record| record.path == LARGE_A && record.status == 206)
        .expect("a resumed request");
    assert_eq!(resumed.range_from, Some(partial));

    let path = fixture.queue.path(second).expect("a cached path");
    assert_eq!(
        std::fs::read(&path).expect("the cached file"),
        fixture.large_a
    );
}

#[test]
fn a_wrong_digest_fails_and_evicts_the_cache_entry() {
    let fixture = Fixture::new();
    let wrong = ArtifactDigest::from_hex(&"0".repeat(64)).expect("a 64-character digest");
    let id = fixture.queue(Request {
        digest: wrong,
        ..fixture.request(SMALL, Priority::User)
    });

    assert_eq!(fixture.wait_terminal(id), State::Failed);
    assert!(matches!(
        fixture.queue.failure(id),
        Some(crate::queue::Failure::Digest { .. })
    ));

    // The poisoned bytes must not survive. If they did, the next attempt would
    // take a cache hit and re-serve them forever.
    let cache = fixture.queue.transport().cache();
    assert!(!cache.is_complete(&wrong));
    assert!(!cache.has_partial(&wrong));
    assert!(!cache.path_for(&wrong).exists());
}

#[test]
fn a_wrong_size_fails_and_evicts_the_cache_entry() {
    let fixture = Fixture::new();
    let digest = digest_of(&fixture.small);
    let id = fixture.queue(Request {
        expected_size: Some(len_of(&fixture.small) + 1),
        ..fixture.request(SMALL, Priority::User)
    });

    assert_eq!(fixture.wait_terminal(id), State::Failed);
    assert_eq!(
        fixture.queue.failure(id),
        Some(crate::queue::Failure::Size {
            expected: len_of(&fixture.small) + 1,
            actual: len_of(&fixture.small),
        })
    );

    let cache = fixture.queue.transport().cache();
    assert!(!cache.is_complete(&digest));
    assert!(!cache.path_for(&digest).exists());
}

#[test]
fn a_pending_entry_cancels_and_frees_its_slot() {
    let fixture = Fixture::new();
    // The one worker is busy for the whole of the throttled body, so the second
    // entry stays pending for the length of this test.
    let active = fixture.queue(fixture.request(LARGE_A, Priority::Background));
    assert!(fixture.wait_state(active, State::Downloading));

    let pending = fixture.queue(fixture.request(SMALL, Priority::Background));
    assert_eq!(fixture.queue.state(pending), State::Pending);

    assert!(fixture.queue.cancel(pending));
    assert_eq!(fixture.queue.state(pending), State::Cancelled);
    assert!(fixture.queue.remove(pending));

    // The active entry keeps its slot until it settles.
    assert!(!fixture.queue.remove(active));
    assert!(!fixture.queue.has_user_pending());
}

#[test]
fn resume_is_refused_unless_the_entry_is_paused() {
    let fixture = Fixture::new();
    let id = fixture.queue(fixture.request(SMALL, Priority::User));
    assert!(!fixture.queue.resume(id));
    assert_eq!(fixture.wait_terminal(id), State::Complete);
    assert!(!fixture.queue.resume(id));
}

/// A cancel that reports success must stick: nothing may later publish
/// `Complete` over an entry the caller was told was cancelled.
///
/// Run against a deep queue and a warm cache, so cancels arrive while the
/// worker is claiming slot after slot. This exercises the invariant under
/// contention; it is not fine-grained enough to land inside the claim window
/// itself, which is a few microseconds wide.
#[test]
fn a_cancel_that_reports_success_is_always_honoured() {
    let fixture = Fixture::new();

    // Warm the cache, so draining an entry is a few syscalls rather than a
    // transfer and the worker walks the queue fast.
    let warm = fixture.queue(fixture.request(SMALL, Priority::User));
    assert_eq!(fixture.wait_terminal(warm), State::Complete);
    assert!(fixture.queue.remove(warm));

    for round in 0..16 {
        let ids: Vec<EntryId> = (0..QUEUE_MAX)
            .map(|_| fixture.queue(fixture.request(SMALL, Priority::User)))
            .collect();
        let promised: Vec<EntryId> = ids
            .iter()
            .copied()
            .filter(|&id| fixture.queue.cancel(id))
            .collect();

        for id in promised {
            assert_eq!(
                fixture.wait_terminal(id),
                State::Cancelled,
                "round {round}: cancel reported success but the transfer ran anyway"
            );
        }
        for id in ids {
            assert!(matches!(
                fixture.wait_terminal(id),
                State::Cancelled | State::Complete
            ));
            assert!(fixture.queue.remove(id));
        }
    }
}

/// The worker-start hook is arbitrary caller code. One that panics must cost
/// its own worker and nothing else: the queue has to release that worker's
/// place and arm a replacement, rather than believing a worker is still armed
/// and never draining again.
///
/// The panic is caught inside the worker, so this test prints a panic message
/// it is expected to survive.
#[test]
fn a_panicking_worker_hook_does_not_wedge_the_queue() {
    let first_start = Arc::new(AtomicBool::new(true));
    let hook_flag = Arc::clone(&first_start);
    let fixture = Fixture::with_config(Config {
        on_worker_start: Some(Arc::new(move |_| {
            assert!(
                !hook_flag.swap(false, Ordering::SeqCst),
                "deliberate panic from the first worker's start hook"
            );
        })),
        ..Config::default()
    });

    let wedged = fixture.queue(fixture.request(SMALL, Priority::User));
    // The first worker dies in its hook, so nothing drains until the next queue
    // call arms a replacement.
    assert!(wait_for(Duration::from_secs(2), || !first_start
        .load(Ordering::SeqCst)));

    let after = fixture.queue(fixture.request(LARGE_A, Priority::User));
    assert_eq!(fixture.wait_terminal(after), State::Complete);
    assert_eq!(fixture.wait_terminal(wedged), State::Complete);
}

/// A resumed transfer seeds its in-flight digest from the bytes already on
/// disk. When those bytes cannot be read back there is nothing to prove what
/// the transfer would be splicing onto, so the entry fails and its partial
/// cache entry is dropped rather than resumed blind.
///
/// Unix only: the fault is injected by making the partial file write-only,
/// which is exactly what lets the transport re-open it for append while the
/// digest seeding cannot read it. Root ignores the permission bits, so the
/// precondition is asserted rather than assumed.
#[cfg(unix)]
#[test]
fn a_partial_file_that_cannot_be_read_back_fails_and_is_evicted() {
    use std::os::unix::fs::PermissionsExt;

    let fixture = Fixture::new();
    let digest = digest_of(&fixture.large_a);
    let cache = fixture.queue.transport().cache().clone();

    let first = fixture.queue(fixture.request(LARGE_A, Priority::User));
    assert!(wait_for(TERMINAL_TIMEOUT, || {
        fixture.queue.state(first) == State::Downloading && fixture.queue.progress(first) > 0.10
    }));
    assert!(fixture.queue.pause(first));
    assert!(fixture.wait_state(first, State::Paused));

    let path = cache.path_for(&digest);
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o200))
        .expect("the partial file's mode is settable");
    assert!(
        std::fs::File::open(&path).is_err(),
        "this test injects its fault with file permissions, which root ignores"
    );

    assert!(fixture.queue.cancel(first));
    assert!(fixture.wait_state(first, State::Cancelled));
    assert!(fixture.queue.remove(first));

    let second = fixture.queue(fixture.request(LARGE_A, Priority::User));
    assert_eq!(fixture.wait_terminal(second), State::Failed);
    assert_eq!(
        fixture.queue.failure(second),
        Some(crate::queue::Failure::Unreadable)
    );

    // The unreadable partial must not survive to be resumed from again.
    assert!(!cache.is_complete(&digest));
    assert!(!cache.has_partial(&digest));
    assert!(!path.exists());
}

#[test]
fn only_the_tls_only_bounded_get_refuses_a_plaintext_host() {
    let body = body_bytes(SMALL_SIZE, 9);
    let routes = HashMap::from([(
        SMALL.to_string(),
        Body::instant(body.clone(), "\"bounded\""),
    )]);
    let server = FixtureServer::start(routes);
    let dir = tempfile::tempdir().expect("a scratch cache directory");
    let transport = Transport::new(dir.path());
    let url = server.url(SMALL);

    assert_eq!(
        transport.get_bounded(&url, SMALL_SIZE).unwrap().body,
        body,
        "the ordinary bounded GET speaks to whoever answers"
    );
    // Which is why the update check's clock uses the other one: an https base
    // that redirects to a plaintext host must yield no Date at all, and the
    // refusal has to come from the transport rather than from a scheme check on
    // the URL the caller started with.
    assert!(transport.get_bounded_over_tls(&url, SMALL_SIZE).is_err());
}
