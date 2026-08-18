//! A priority queue of validated transfers, drained by owned worker threads.
//!
//! Each entry's bytes are checked against the digest and length its request
//! named before it counts as complete, so one corrupt artifact fails on its
//! own and takes its cache entry with it. A [`Priority::User`] entry preempts
//! an in-flight [`Priority::Background`] one, and the preempted entry
//! auto-resumes once the higher-priority work settles.
//!
//! **Worker model.** The crate owns its threads: no executor is injected. A
//! drain worker is armed lazily and **re-arms itself** while the queue has
//! work, retiring when it drains. [`TransferQueue::queue`] and
//! [`TransferQueue::resume`] arm under the same lock the worker retires under,
//! so work arriving as a worker retires cannot be missed. A long transfer
//! occupies its worker for the duration.

mod entry;
mod validate;

use std::panic::{self, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicI64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{SystemTime, UNIX_EPOCH};

use sha2::{Digest as _, Sha256};

use crate::transport::{ArtifactDigest, Chunk, Download, Snapshot, Status, Transport};
use entry::{Entry, UNKNOWN_TOTAL, auto_resume_preempted, next_pending, preempt_active_for_user};
use validate::{CHUNK_SIZE, hash_prefix, validate};

pub use entry::{Priority, State};
pub use validate::Failure;

/// The most entries the queue holds at once.
pub const QUEUE_MAX: usize = 32;

/// Workers this revision runs.
///
/// The queue tracks a single active transfer, so draining with more than one
/// worker is a design change rather than a copy; a larger [`Config::workers`]
/// is pinned down to this.
pub const MAX_WORKERS: usize = 1;

/// Run at the top of every worker thread, given that worker's index.
///
/// The caller uses it for whatever the host needs the thread to be — CPU
/// affinity, scheduling priority, a name in a debugger.
pub type WorkerHook = Arc<dyn Fn(usize) + Send + Sync>;

/// How a [`TransferQueue`] runs its workers.
#[derive(Clone)]
pub struct Config {
    /// Workers draining the queue, capped at [`MAX_WORKERS`]. Zero reads as
    /// one.
    pub workers: usize,
    /// Run at the top of every worker thread.
    pub on_worker_start: Option<WorkerHook>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            workers: MAX_WORKERS,
            on_worker_start: None,
        }
    }
}

/// One artifact to acquire.
#[derive(Clone, Debug)]
pub struct Request {
    /// Where to fetch it from.
    pub url: String,
    /// The digest its bytes must have, and the key of its cache entry.
    pub digest: ArtifactDigest,
    /// The length its bytes must have, when the manifest named one.
    pub expected_size: Option<u64>,
    /// How it is scheduled against the rest of the queue.
    pub priority: Priority,
}

/// A handle to one queued transfer.
///
/// Indexes the queue's fixed slot array, and is only valid for the queue that
/// issued it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct EntryId(usize);

/// What a request carries once it is in a slot.
#[derive(Clone)]
struct SlotRequest {
    url: String,
    digest: ArtifactDigest,
    expected_size: Option<u64>,
}

/// One slot in the fixed queue.
///
/// The scheduling fields are atomics, read by both the caller and the worker.
/// The request is written once under the queue lock before the slot becomes
/// eligible; the two published results are written by the worker and read by
/// the caller under their own locks.
#[derive(Default)]
struct Slot {
    in_use: AtomicBool,
    priority: AtomicI32,
    state: AtomicI32,
    cancel_requested: AtomicBool,
    queue_time_ms: AtomicI64,
    bytes_downloaded: AtomicI64,
    bytes_total: AtomicI64,
    request: Mutex<Option<SlotRequest>>,
    path: Mutex<Option<PathBuf>>,
    failure: Mutex<Option<Failure>>,
}

impl Slot {
    /// The scheduling view the pure policy functions operate on.
    fn snapshot(&self) -> Entry {
        Entry {
            in_use: self.in_use.load(Ordering::Acquire),
            priority: Priority::from_code(self.priority.load(Ordering::Acquire)),
            state: State::from_code(self.state.load(Ordering::Acquire)),
            queue_time_ms: self.queue_time_ms.load(Ordering::Acquire),
            bytes_downloaded: self.bytes_downloaded.load(Ordering::Acquire),
            bytes_total: self.bytes_total.load(Ordering::Acquire),
        }
    }

    /// Write a policy decision's mutated fields back.
    fn apply(&self, entry: &Entry) {
        self.in_use.store(entry.in_use, Ordering::Release);
        self.state.store(entry.state.code(), Ordering::Release);
    }

    fn state(&self) -> State {
        State::from_code(self.state.load(Ordering::Acquire))
    }

    fn set_state(&self, state: State) {
        self.state.store(state.code(), Ordering::Release);
    }

    fn request(&self) -> Option<SlotRequest> {
        lock(&self.request).clone()
    }

    fn store_progress(&self, snapshot: &Snapshot) {
        self.bytes_downloaded
            .store(as_i64(snapshot.downloaded), Ordering::Release);
        self.bytes_total.store(
            snapshot.total.map_or(UNKNOWN_TOTAL, as_i64),
            Ordering::Release,
        );
    }
}

/// Everything the caller and the workers share. Held behind an `Arc` so a
/// worker keeps a valid view even while the caller is inside `drop`.
struct Shared {
    transport: Transport,
    slots: Vec<Slot>,
    /// Serializes queue mutation against worker arm/retire, so work that
    /// arrives as a worker retires still arms a fresh one.
    queue_lock: Mutex<WorkerState>,
    running: AtomicBool,
    /// Index of the slot a worker currently owns, biased by one so `0` means
    /// "no active slot".
    active_slot: AtomicUsize,
    active_download: Mutex<Option<Arc<Download>>>,
    workers: usize,
    on_worker_start: Option<WorkerHook>,
}

/// How many drain workers are armed, and their handles so `drop` can join
/// them.
#[derive(Default)]
struct WorkerState {
    armed: usize,
    handles: Vec<JoinHandle<()>>,
}

impl Shared {
    fn active_index(&self) -> Option<usize> {
        match self.active_slot.load(Ordering::Acquire) {
            0 => None,
            biased => Some(biased - 1),
        }
    }

    fn set_active(&self, index: Option<usize>) {
        self.active_slot
            .store(index.map_or(0, |i| i + 1), Ordering::Release);
    }

    fn active_download(&self) -> Option<Arc<Download>> {
        lock(&self.active_download).clone()
    }

    /// Whether `index` is the slot a worker currently owns.
    fn is_active(&self, index: usize) -> bool {
        self.active_index() == Some(index)
    }

    fn snapshot_all(&self) -> Vec<Entry> {
        self.slots.iter().map(Slot::snapshot).collect()
    }

    fn queue_lock(&self) -> MutexGuard<'_, WorkerState> {
        lock(&self.queue_lock)
    }
}

/// A prior panic left this state usable, so recover rather than propagate.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

fn as_i64(value: u64) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

/// Wall-clock milliseconds since the Unix epoch, the tie-breaker between
/// entries queued at the same priority.
fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| {
            as_i64(since.as_millis().try_into().unwrap_or(u64::MAX))
        })
}

// Workers.

/// Arm drain workers up to the configured count, if there is pending work.
///
/// Called with `state` being the held queue lock, so this cannot race a
/// worker's own retire.
fn arm_workers(shared: &Arc<Shared>, state: &mut WorkerState) {
    if state.armed >= shared.workers || !shared.running.load(Ordering::Acquire) {
        return;
    }
    if next_pending(&shared.snapshot_all()).is_none() {
        return;
    }
    reap(state);
    while state.armed < shared.workers {
        let index = state.armed;
        let worker = Arc::clone(shared);
        let spawned = thread::Builder::new()
            .name(format!("retrovert-transfer-{index}"))
            .spawn(move || run_worker(&worker, index));
        let Ok(handle) = spawned else {
            // Leave `armed` where it is so a later queue tries again.
            return;
        };
        state.handles.push(handle);
        state.armed += 1;
    }
}

/// Join workers that have already retired. A retired worker is past its last
/// use of the queue lock, so joining while holding that lock cannot deadlock.
fn reap(state: &mut WorkerState) {
    let (finished, live): (Vec<_>, Vec<_>) =
        state.handles.drain(..).partition(JoinHandle::is_finished);
    state.handles = live;
    for handle in finished {
        let _ = handle.join();
    }
}

/// One worker's whole life: the caller's start hook, then the drain.
///
/// [`run_drain`] releases this worker's share of `armed` under the queue lock
/// it retires with. An unwind skips that — the start hook is arbitrary caller
/// code — so it is released here instead. Without this, `armed` never falls
/// and the queue silently stops arming workers.
fn run_worker(shared: &Arc<Shared>, index: usize) {
    let drained = panic::catch_unwind(AssertUnwindSafe(|| {
        if let Some(hook) = shared.on_worker_start.as_ref() {
            hook(index);
        }
        run_drain(shared);
    }));
    if drained.is_err() {
        shared.queue_lock().armed -= 1;
    }
}

/// Drain the queue in priority order until nothing is pending or the queue is
/// shutting down.
fn run_drain(shared: &Arc<Shared>) {
    loop {
        if !shared.running.load(Ordering::Acquire) {
            shared.queue_lock().armed -= 1;
            return;
        }

        let index = {
            let mut state = shared.queue_lock();
            let Some(index) = next_pending(&shared.snapshot_all()) else {
                // Retire under the lock: a `queue` racing this either lands
                // before it, and the scan sees the work, or after it, and arms
                // a fresh worker.
                state.armed -= 1;
                return;
            };
            // Claim the slot before releasing the lock. While it still reads
            // Pending, a cancel or a remove cannot tell it from unclaimed work:
            // a cancel would report success over a transfer that then runs
            // anyway, and a remove would free the slot for a `queue` to reuse
            // under this worker's feet.
            shared.slots[index].set_state(State::Downloading);
            shared.set_active(Some(index));
            index
        };

        // A panicking transfer still has to settle its slot and drop the active
        // handles, or the claim above wedges the queue against every later
        // entry.
        if panic::catch_unwind(AssertUnwindSafe(|| process(shared, index))).is_err() {
            finish(
                shared,
                index,
                None,
                None,
                State::Failed,
                Some(Failure::Panicked),
            );
        }

        // A preempted entry auto-resumes once the higher-priority work has
        // settled; an explicitly paused one stays paused.
        {
            let _state = shared.queue_lock();
            let mut snapshot = shared.snapshot_all();
            auto_resume_preempted(&mut snapshot);
            for (slot, entry) in shared.slots.iter().zip(snapshot.iter()) {
                slot.apply(entry);
            }
        }
    }
}

/// Run one slot's transfer to a terminal state.
fn process(shared: &Arc<Shared>, index: usize) {
    let slot = &shared.slots[index];
    let Some(request) = slot.request() else {
        finish(shared, index, None, None, State::Failed, None);
        return;
    };

    // A cancel that landed in the window between the slot being claimed and the
    // transfer starting, so no request goes out for work already called off.
    if slot.cancel_requested.load(Ordering::Acquire) {
        finish(shared, index, None, None, State::Cancelled, None);
        return;
    }

    // A complete cache entry passed these same checks when it landed, so
    // serving it needs neither the network nor a re-hash.
    let cache_path = shared.transport.cache().path_for(&request.digest);
    if shared.transport.cache().is_complete(&request.digest) {
        let cached = std::fs::metadata(&cache_path).map_or(0, |meta| as_i64(meta.len()));
        slot.bytes_downloaded.store(cached, Ordering::Release);
        slot.bytes_total.store(cached, Ordering::Release);
        finish(
            shared,
            index,
            Some(&request.digest),
            None,
            State::Complete,
            None,
        );
        return;
    }

    let download = match shared
        .transport
        .download(&request.url, &request.digest, true)
    {
        Ok(download) => Arc::new(download),
        Err(error) => {
            finish(
                shared,
                index,
                None,
                None,
                State::Failed,
                Some(Failure::Start(error.to_string())),
            );
            return;
        }
    };

    // A cancel that arrived while the transport was being set up.
    if slot.cancel_requested.load(Ordering::Acquire) {
        finish(shared, index, None, Some(download), State::Cancelled, None);
        return;
    }

    *lock(&shared.active_download) = Some(Arc::clone(&download));

    // Re-check: a cancel could have landed between the check above and the
    // handle becoming visible to `cancel`.
    if slot.cancel_requested.load(Ordering::Acquire) {
        download.request_cancel();
    }

    transfer(shared, index, &request, &download, &cache_path);
}

/// Stream a started transfer to a terminal state and settle its slot.
fn transfer(
    shared: &Arc<Shared>,
    index: usize,
    request: &SlotRequest,
    download: &Arc<Download>,
    cache_path: &Path,
) {
    let slot = &shared.slots[index];
    let mut digest = Sha256::new();
    let resumed_from = download.progress().downloaded;
    if resumed_from > 0 && !hash_prefix(cache_path, resumed_from, &mut digest) {
        // The partial file cannot be read back, so nothing can be proven about
        // what a resumed transfer would splice onto. Drop it and start over —
        // after `finish` has released the transfer's own handle on it.
        finish(
            shared,
            index,
            None,
            Some(Arc::clone(download)),
            State::Failed,
            Some(Failure::Unreadable),
        );
        shared.transport.cache().evict(&request.digest);
        return;
    }

    let mut buffer = vec![0u8; CHUNK_SIZE];
    loop {
        let chunk = download.read_chunk(&mut buffer);
        slot.store_progress(&download.progress());
        match chunk {
            Chunk::Read(read) => digest.update(&buffer[..read]),
            Chunk::Idle | Chunk::Failed => break,
        }
    }

    let snapshot = download.progress();
    let mut failure = None;
    let state = match snapshot.status {
        Status::Complete => {
            match validate(
                cache_path,
                &request.digest,
                request.expected_size,
                digest,
                snapshot.downloaded,
            ) {
                Ok(()) => State::Complete,
                Err(refused) => {
                    // The cache entry holds bytes that failed validation. Drop
                    // it, or the next attempt takes a cache hit and re-serves
                    // the same poisoned bytes forever.
                    shared.transport.cache().evict(&request.digest);
                    failure = Some(refused);
                    State::Failed
                }
            }
        }
        Status::Cancelled => State::Cancelled,
        Status::Paused => State::Paused,
        Status::Pending | Status::Downloading | Status::Failed => {
            failure = snapshot.failure.map(Failure::Transfer);
            State::Failed
        }
    };

    finish(
        shared,
        index,
        Some(&request.digest),
        Some(Arc::clone(download)),
        state,
        failure,
    );
}

/// Settle a slot: release the active handles, publish the cached path or the
/// failure, and publish the terminal state.
///
/// `Preempted` survives a `Paused` settle, so the auto-resume sweep still picks
/// the slot up.
fn finish(
    shared: &Arc<Shared>,
    index: usize,
    digest: Option<&ArtifactDigest>,
    download: Option<Arc<Download>>,
    state: State,
    failure: Option<Failure>,
) {
    // Clear the active handles under the queue lock before dropping the
    // transfer, so a concurrent cancel / pause / preempt either completed its
    // call already or observes the cleared slot.
    {
        let _guard = shared.queue_lock();
        *lock(&shared.active_download) = None;
        shared.set_active(None);
    }
    drop(download);

    let slot = &shared.slots[index];
    if let (State::Complete, Some(digest)) = (state, digest) {
        *lock(&slot.path) = Some(shared.transport.cache().path_for(digest));
    } else if let Some(failure) = failure {
        publish_failure(slot, failure);
    }

    let already_preempted = slot.state() == State::Preempted && state == State::Paused;
    if !already_preempted {
        slot.set_state(state);
    }
}

/// Record a failure, keeping the first one reported for this attempt.
fn publish_failure(slot: &Slot, failure: Failure) {
    let mut current = lock(&slot.failure);
    if current.is_none() {
        *current = Some(failure);
    }
}

/// A priority queue of validated transfers, drained by owned worker threads.
pub struct TransferQueue {
    shared: Arc<Shared>,
}

impl TransferQueue {
    /// A queue acquiring artifacts through `transport`, with the default
    /// worker configuration.
    #[must_use]
    pub fn new(transport: Transport) -> Self {
        Self::with_config(transport, Config::default())
    }

    /// A queue acquiring artifacts through `transport`, configured by
    /// `config`.
    #[must_use]
    pub fn with_config(transport: Transport, config: Config) -> Self {
        let mut slots = Vec::with_capacity(QUEUE_MAX);
        slots.resize_with(QUEUE_MAX, Slot::default);
        Self {
            shared: Arc::new(Shared {
                transport,
                slots,
                queue_lock: Mutex::new(WorkerState::default()),
                running: AtomicBool::new(true),
                active_slot: AtomicUsize::new(0),
                active_download: Mutex::new(None),
                workers: config.workers.clamp(1, MAX_WORKERS),
                on_worker_start: config.on_worker_start,
            }),
        }
    }

    /// The transport this queue acquires through, and the cache it fills.
    #[must_use]
    pub fn transport(&self) -> &Transport {
        &self.shared.transport
    }

    /// Workers this queue drains with.
    #[must_use]
    pub fn workers(&self) -> usize {
        self.shared.workers
    }

    /// Queue a transfer, or `None` when all [`QUEUE_MAX`] slots are in use.
    ///
    /// A [`Priority::User`] request preempts an in-flight
    /// [`Priority::Background`] one, which pauses and auto-resumes once the
    /// higher-priority work settles.
    #[must_use]
    pub fn queue(&self, request: Request) -> Option<EntryId> {
        let shared = &self.shared;
        let mut state = shared.queue_lock();

        let index = shared
            .slots
            .iter()
            .position(|slot| !slot.in_use.load(Ordering::Acquire))?;
        let slot = &shared.slots[index];
        let priority = request.priority;

        *lock(&slot.request) = Some(SlotRequest {
            url: request.url,
            digest: request.digest,
            expected_size: request.expected_size,
        });
        *lock(&slot.path) = None;
        *lock(&slot.failure) = None;
        slot.priority.store(priority.code(), Ordering::Release);
        slot.cancel_requested.store(false, Ordering::Release);
        slot.bytes_downloaded.store(0, Ordering::Release);
        slot.bytes_total.store(UNKNOWN_TOTAL, Ordering::Release);
        slot.queue_time_ms.store(now_ms(), Ordering::Release);
        slot.set_state(State::Pending);
        slot.in_use.store(true, Ordering::Release);

        if let Some(active_index) = shared.active_index() {
            let active = &shared.slots[active_index];
            let mut snapshot = active.snapshot();
            if preempt_active_for_user(Some(&mut snapshot), priority) {
                active.apply(&snapshot);
                if let Some(download) = shared.active_download() {
                    download.request_pause();
                }
            }
        }

        arm_workers(shared, &mut state);
        Some(EntryId(index))
    }

    /// Cancel a queued or active transfer. Returns whether the request
    /// applied.
    #[must_use]
    pub fn cancel(&self, id: EntryId) -> bool {
        let shared = &self.shared;
        let slot = &shared.slots[id.0];
        let _guard = shared.queue_lock();
        let mut snapshot = slot.snapshot();
        if !entry::cancel(&mut snapshot, shared.is_active(id.0)) {
            return false;
        }
        slot.apply(&snapshot);
        // An active transfer is asked to stop. If it has no handle yet — still
        // inside the transport's first request — the flag is picked up by the
        // worker instead.
        if snapshot.state == State::Downloading {
            match shared.active_download() {
                Some(download) => download.request_cancel(),
                None => slot.cancel_requested.store(true, Ordering::Release),
            }
        }
        true
    }

    /// Pause an active transfer, keeping its partial file resumable. Returns
    /// whether the request applied.
    #[must_use]
    pub fn pause(&self, id: EntryId) -> bool {
        let shared = &self.shared;
        let _guard = shared.queue_lock();
        if !entry::pause(&shared.slots[id.0].snapshot(), shared.is_active(id.0)) {
            return false;
        }
        if let Some(download) = shared.active_download() {
            download.request_pause();
        }
        true
    }

    /// Resume a paused transfer. Returns whether the request applied.
    #[must_use]
    pub fn resume(&self, id: EntryId) -> bool {
        let shared = &self.shared;
        let slot = &shared.slots[id.0];
        let mut state = shared.queue_lock();
        let mut snapshot = slot.snapshot();
        if !entry::resume(&mut snapshot) {
            return false;
        }
        slot.apply(&snapshot);
        arm_workers(shared, &mut state);
        true
    }

    /// Free a slot. Refused while its transfer is active.
    #[must_use]
    pub fn remove(&self, id: EntryId) -> bool {
        let shared = &self.shared;
        let slot = &shared.slots[id.0];
        let _guard = shared.queue_lock();
        let mut snapshot = slot.snapshot();
        if !entry::remove(&mut snapshot) {
            return false;
        }
        slot.apply(&snapshot);
        true
    }

    /// Where a transfer stands.
    #[must_use]
    pub fn state(&self, id: EntryId) -> State {
        self.shared.slots[id.0].state()
    }

    /// The cache path of a completed transfer; `None` until it completes.
    #[must_use]
    pub fn path(&self, id: EntryId) -> Option<PathBuf> {
        lock(&self.shared.slots[id.0].path).clone()
    }

    /// Why a transfer failed; `None` unless it did.
    #[must_use]
    pub fn failure(&self, id: EntryId) -> Option<Failure> {
        lock(&self.shared.slots[id.0].failure).clone()
    }

    /// Fractional transfer progress, or `0.0` while the total is unknown.
    #[must_use]
    pub fn progress(&self, id: EntryId) -> f32 {
        self.shared.slots[id.0].snapshot().progress()
    }

    /// Whether any queued user-priority transfer is still waiting for a
    /// worker.
    #[must_use]
    pub fn has_user_pending(&self) -> bool {
        entry::has_user_pending(&self.shared.snapshot_all())
    }
}

impl Drop for TransferQueue {
    /// Cancel the active transfer, stop the workers re-arming, and block until
    /// every one of them has exited.
    ///
    /// A cancel is observed between reads, so this waits out the read in
    /// flight. The streaming agent bounds connecting but deliberately not the
    /// body — a large artifact on a slow link is not an error — so a peer that
    /// stops sending without closing holds the drop open.
    fn drop(&mut self) {
        self.shared.running.store(false, Ordering::Release);
        if let Some(download) = self.shared.active_download() {
            download.request_cancel();
        }
        let handles = std::mem::take(&mut self.shared.queue_lock().handles);
        for handle in handles {
            let _ = handle.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn queue_over(dir: &std::path::Path, config: Config) -> TransferQueue {
        TransferQueue::with_config(Transport::new(dir), config)
    }

    #[test]
    fn a_worker_count_above_the_cap_is_pinned_to_it() {
        let dir = tempfile::tempdir().unwrap();
        for requested in [0, 1, MAX_WORKERS + 7] {
            let queue = queue_over(
                dir.path(),
                Config {
                    workers: requested,
                    ..Config::default()
                },
            );
            assert_eq!(queue.workers(), MAX_WORKERS);
        }
    }

    #[test]
    fn the_worker_start_hook_does_not_run_before_there_is_work() {
        let dir = tempfile::tempdir().unwrap();
        let started = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&started);
        let queue = queue_over(
            dir.path(),
            Config {
                on_worker_start: Some(Arc::new(move |_| {
                    counter.fetch_add(1, Ordering::Release);
                })),
                ..Config::default()
            },
        );
        drop(queue);
        assert_eq!(started.load(Ordering::Acquire), 0);
    }

    #[test]
    fn a_queue_with_no_free_slot_refuses_the_next_request() {
        let dir = tempfile::tempdir().unwrap();
        let queue = queue_over(dir.path(), Config::default());

        // Nothing is ever transferred: every slot is filled and then the queue
        // is dropped, which cancels whatever a worker picked up.
        let ids: Vec<_> = (0..QUEUE_MAX)
            .map(|i| queue.queue(request(i)).expect("a free slot per request"))
            .collect();
        assert_eq!(ids.len(), QUEUE_MAX);
        assert!(queue.queue(request(QUEUE_MAX)).is_none());
    }

    fn request(seed: usize) -> Request {
        let mut hex = format!("{seed:064x}");
        hex.truncate(64);
        Request {
            // Never reached: the loopback discard port refuses connections.
            url: format!("http://127.0.0.1:9/{seed}"),
            digest: ArtifactDigest::from_hex(&hex).unwrap(),
            expected_size: None,
            priority: Priority::Background,
        }
    }
}
