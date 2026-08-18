//! The transfer queue against a real HTTP server.
//!
//! The server is in-process and hermetic (see [`fixture_server`]), so these run
//! wherever `cargo test` runs rather than skipping when a LAN host is missing.

mod fixture_server;

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use fixture_server::{Body, FixtureServer};
use retrovert_updater::queue::{
    Config, EntryId, Priority, QUEUE_MAX, Request, State, TransferQueue,
};
use retrovert_updater::transport::{ArtifactDigest, Transport};
use sha2::{Digest, Sha256};

/// Small enough to arrive in one piece.
const SMALL_SIZE: usize = 64 * 1024;
/// Served in pieces slowly enough that a test can catch it mid-transfer.
const LARGE_SIZE: usize = 1024 * 1024;
const LARGE_PIECE: usize = 16 * 1024;
const LARGE_DELAY: Duration = Duration::from_millis(40);

const SMALL: &str = "/small";
const LARGE_A: &str = "/large-a";
const LARGE_B: &str = "/large-b";

/// Long enough to cover a whole throttled body several times over on a loaded
/// CI runner.
const TERMINAL_TIMEOUT: Duration = Duration::from_secs(60);
const POLL: Duration = Duration::from_millis(20);

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
}

impl Fixture {
    fn new() -> Self {
        Self::with_config(Config::default())
    }

    fn with_config(config: Config) -> Self {
        let small = body_bytes(SMALL_SIZE, 1);
        let large_a = body_bytes(LARGE_SIZE, 2);
        let large_b = body_bytes(LARGE_SIZE, 3);

        let mut routes = HashMap::new();
        routes.insert(SMALL.to_string(), Body::instant(small.clone(), "\"small\""));
        for (path, bytes, etag) in [
            (LARGE_A, &large_a, "\"large-a\""),
            (LARGE_B, &large_b, "\"large-b\""),
        ] {
            routes.insert(
                path.to_string(),
                Body::throttled(bytes.clone(), etag, LARGE_PIECE, LARGE_DELAY),
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
        }
    }

    fn bytes(&self, path: &str) -> &[u8] {
        match path {
            SMALL => &self.small,
            LARGE_A => &self.large_a,
            LARGE_B => &self.large_b,
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
        Some(retrovert_updater::queue::Failure::Digest { .. })
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
        Some(retrovert_updater::queue::Failure::Size {
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
        Some(retrovert_updater::queue::Failure::Unreadable)
    );

    // The unreadable partial must not survive to be resumed from again.
    assert!(!cache.is_complete(&digest));
    assert!(!cache.has_partial(&digest));
    assert!(!path.exists());
}
