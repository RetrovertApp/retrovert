//! A priority queue of validated transfers, drained by owned worker threads.
//!
//! Each entry's bytes are checked against the digest and length its request
//! named before it counts as complete, so one corrupt artifact fails on its
//! own and takes its cache entry with it. A [`Priority::User`] entry preempts
//! an in-flight [`Priority::Background`] one, and the preempted entry
//! auto-resumes once the higher-priority work settles.
//!
//! **Worker model.** The crate owns its threads: no executor is injected.
//! Workers are armed lazily up to [`Config::workers`] and re-arm themselves
//! while the queue has work, retiring when it drains. Arming and retiring
//! happen under the same lock, so work arriving as a worker retires cannot be
//! missed. A worker holds one slot at a time.

mod entry;
mod validate;

#[cfg(test)]
mod transfers;

use std::panic::{self, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicI64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{SystemTime, UNIX_EPOCH};

use sha2::{Digest as _, Sha256};

use crate::transport::{ArtifactDigest, Chunk, Download, Snapshot, Status, Transport};
use entry::{Entry, UNKNOWN_TOTAL, auto_resume_preempted, next_pending, preempt_for_user};
use validate::{CHUNK_SIZE, hash_prefix, validate};

pub(crate) use validate::verify_on_disk;

pub use entry::{Priority, State};
pub use validate::Failure;

/// The most entries the queue holds at once.
pub const QUEUE_MAX: usize = 32;

/// The most workers a queue drains with.
///
/// A worker can only ever hold one slot, so more workers than there are slots
/// buys nothing.
pub const MAX_WORKERS: usize = QUEUE_MAX;

/// Run at the top of every worker thread, given that worker's index.
///
/// The caller uses it for whatever the host needs the thread to be — CPU
/// affinity, scheduling priority, a name in a debugger.
pub type WorkerHook = Arc<dyn Fn(usize) + Send + Sync>;

/// How a [`TransferQueue`] runs its workers.
#[derive(Clone)]
pub struct Config {
    /// Workers draining the queue, and so transfers in flight at once. Clamped
    /// to `1..=`[`MAX_WORKERS`].
    pub workers: usize,
    /// Run at the top of every worker thread.
    pub on_worker_start: Option<WorkerHook>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            workers: 1,
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
///
/// A slot reads Downloading from the claim, but the transfer only becomes
/// reachable once the transport has started it. The request flags carry a
/// cancel or a pause across that window.
#[derive(Default)]
struct Slot {
    in_use: AtomicBool,
    priority: AtomicI32,
    state: AtomicI32,
    cancel_requested: AtomicBool,
    pause_requested: AtomicBool,
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
    /// Serializes queue mutation against worker arm/claim/retire, so work that
    /// arrives as a worker retires still arms a fresh one.
    queue_lock: Mutex<WorkerState>,
    running: AtomicBool,
    workers: usize,
    on_worker_start: Option<WorkerHook>,
}

/// Which workers are armed, what each is transferring, and their handles so
/// `drop` can join them.
///
/// The per-worker records live under the queue lock rather than beside it, so
/// a caller deciding against the active set sees it as one consistent whole.
struct WorkerState {
    armed: Vec<bool>,
    active: Vec<Active>,
    handles: Vec<JoinHandle<()>>,
}

/// The slot one worker holds, and the transfer running in it.
#[derive(Default)]
struct Active {
    slot: Option<usize>,
    download: Option<Arc<Download>>,
}

impl WorkerState {
    fn new(workers: usize) -> Self {
        let mut active = Vec::with_capacity(workers);
        active.resize_with(workers, Active::default);
        Self {
            armed: vec![false; workers],
            active,
            handles: Vec::new(),
        }
    }

    fn claimed(&self) -> Vec<usize> {
        self.active.iter().filter_map(|a| a.slot).collect()
    }

    fn is_active(&self, index: usize) -> bool {
        self.active.iter().any(|a| a.slot == Some(index))
    }

    /// `None` until the worker holding `index` has started its transfer.
    fn download_for(&self, index: usize) -> Option<Arc<Download>> {
        self.active
            .iter()
            .find(|a| a.slot == Some(index))
            .and_then(|a| a.download.clone())
    }

    fn has_idle_worker(&self) -> bool {
        self.active.iter().any(|a| a.slot.is_none())
    }
}

impl Shared {
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
    if state.armed.iter().all(|&armed| armed) || !shared.running.load(Ordering::Acquire) {
        return;
    }
    if next_pending(&shared.snapshot_all()).is_none() {
        return;
    }
    reap(state);
    for index in 0..shared.workers {
        if state.armed[index] {
            continue;
        }
        let worker = Arc::clone(shared);
        let spawned = thread::Builder::new()
            .name(format!("retrovert-transfer-{index}"))
            .spawn(move || run_worker(&worker, index));
        let Ok(handle) = spawned else {
            // Leave this index unarmed so a later queue tries again.
            return;
        };
        state.handles.push(handle);
        state.armed[index] = true;
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
/// An unwind skips the orderly retire in [`run_drain`] — the start hook is
/// arbitrary caller code — leaving the worker armed forever and the queue
/// unable to replace it, so it is released here instead.
fn run_worker(shared: &Arc<Shared>, index: usize) {
    let drained = panic::catch_unwind(AssertUnwindSafe(|| {
        if let Some(hook) = shared.on_worker_start.as_ref() {
            hook(index);
        }
        run_drain(shared, index);
    }));
    if drained.is_err() {
        release_worker(shared, index);
    }
}

/// Disarm a worker that unwound past its orderly retire, failing whatever slot
/// it still held rather than leaving it claimed by a thread that is gone.
fn release_worker(shared: &Arc<Shared>, index: usize) {
    let released = {
        let mut state = shared.queue_lock();
        state.armed[index] = false;
        let released = std::mem::take(&mut state.active[index]);
        // Under the lock: the resume sweep writes back a whole-array snapshot,
        // and would otherwise revert this to Downloading with no worker behind
        // it.
        if let Some(claimed) = released.slot {
            publish_failure(&shared.slots[claimed], Failure::Panicked);
            shared.slots[claimed].set_state(State::Failed);
        }
        released
    };
    drop(released);
}

/// Return every preempted entry to Pending, once no user-priority work is left
/// to run. Call with the queue lock held.
fn resume_preempted(shared: &Arc<Shared>) {
    let mut snapshot = shared.snapshot_all();
    auto_resume_preempted(&mut snapshot);
    for (slot, entry) in shared.slots.iter().zip(snapshot.iter()) {
        slot.apply(entry);
    }
}

/// Drain the queue in priority order until nothing is pending or the queue is
/// shutting down.
fn run_drain(shared: &Arc<Shared>, worker: usize) {
    loop {
        if !shared.running.load(Ordering::Acquire) {
            shared.queue_lock().armed[worker] = false;
            return;
        }

        let index = {
            let mut state = shared.queue_lock();
            // Resuming and deciding must be one lock hold. Split, the last
            // user entry can be cancelled in between — after the sweep refused
            // to resume on its account, before this worker retires on top of
            // the preempted entry — and nothing sweeps again.
            resume_preempted(shared);
            let Some(index) = next_pending(&shared.snapshot_all()) else {
                // Retire under the lock: a `queue` racing this either lands
                // before it, and the scan sees the work, or after it, and arms
                // a fresh worker.
                state.armed[worker] = false;
                return;
            };
            // Claim the slot before releasing the lock. While it still reads
            // Pending, a cancel or a remove cannot tell it from unclaimed work:
            // a cancel would report success over a transfer that then runs
            // anyway, and a remove would free the slot for a `queue` to reuse
            // under this worker's feet. Claiming it also keeps a second worker
            // off it, since only a Pending slot is eligible.
            shared.slots[index].set_state(State::Downloading);
            state.active[worker].slot = Some(index);
            // What the sweep released can be more than this worker can carry.
            arm_workers(shared, &mut state);
            index
        };

        // A panicking transfer still has to settle its slot and drop the active
        // handles, or the claim above wedges the queue against every later
        // entry.
        if panic::catch_unwind(AssertUnwindSafe(|| process(shared, worker, index))).is_err() {
            finish(
                shared,
                worker,
                index,
                None,
                None,
                State::Failed,
                Some(Failure::Panicked),
            );
        }
    }
}

/// Run one slot's transfer to a terminal state.
fn process(shared: &Arc<Shared>, worker: usize, index: usize) {
    let slot = &shared.slots[index];
    let Some(request) = slot.request() else {
        finish(shared, worker, index, None, None, State::Failed, None);
        return;
    };

    // A cancel that landed in the window between the slot being claimed and the
    // transfer starting, so no request goes out for work already called off.
    if slot.cancel_requested.load(Ordering::Acquire) {
        finish(shared, worker, index, None, None, State::Cancelled, None);
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
            worker,
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
                worker,
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
        finish(
            shared,
            worker,
            index,
            None,
            Some(download),
            State::Cancelled,
            None,
        );
        return;
    }

    shared.queue_lock().active[worker].download = Some(Arc::clone(&download));

    // Anything that landed before the handle became visible. A missed pause
    // leaves the entry marked Preempted while it transfers on, holding the
    // worker the user request was queued to get; a missed shutdown holds
    // `drop` open for a whole body.
    if slot.cancel_requested.load(Ordering::Acquire) || !shared.running.load(Ordering::Acquire) {
        download.request_cancel();
    }
    if slot.pause_requested.load(Ordering::Acquire) {
        download.request_pause();
    }

    transfer(shared, worker, index, &request, &download, &cache_path);
}

/// Stream a started transfer to a terminal state and settle its slot.
fn transfer(
    shared: &Arc<Shared>,
    worker: usize,
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
            worker,
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
    // The request named a length, so a body that keeps going past it is
    // stopped here rather than read to whatever end the server picks: nothing
    // the extra bytes could hash to would validate, and an unbounded body
    // would otherwise fill the disk.
    let mut oversize = None;
    loop {
        let chunk = download.read_chunk(&mut buffer);
        slot.store_progress(&download.progress());
        match chunk {
            Chunk::Read(read) => {
                digest.update(&buffer[..read]);
                if let Some(expected) = request.expected_size {
                    let downloaded = download.progress().downloaded;
                    if downloaded > expected && oversize.is_none() {
                        oversize = Some(Failure::Oversize {
                            expected,
                            actual: downloaded,
                        });
                        download.request_cancel();
                    }
                }
            }
            Chunk::Idle | Chunk::Failed => break,
        }
    }

    let snapshot = download.progress();
    let mut failure = None;
    let state = if let Some(refused) = oversize {
        // The cancel above discards the partial file; eviction also clears any
        // sidecar an earlier pause left, so nothing resumes onto refused bytes.
        shared.transport.cache().evict(&request.digest);
        failure = Some(refused);
        State::Failed
    } else {
        match snapshot.status {
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
        }
    };

    finish(
        shared,
        worker,
        index,
        Some(&request.digest),
        Some(Arc::clone(download)),
        state,
        failure,
    );
}

/// Settle a slot: release the worker's claim, publish the cached path or the
/// failure, and publish the terminal state.
///
/// All under the queue lock, so a cancel either lands before this and leaves
/// `cancel_requested` for the settle to honour, or lands after and finds the
/// slot inactive and refuses. `Preempted` survives a `Paused` settle, so the
/// resume sweep still picks the slot up.
fn finish(
    shared: &Arc<Shared>,
    worker: usize,
    index: usize,
    digest: Option<&ArtifactDigest>,
    download: Option<Arc<Download>>,
    state: State,
    failure: Option<Failure>,
) {
    let slot = &shared.slots[index];
    let released = {
        let mut guard = shared.queue_lock();
        let released = std::mem::take(&mut guard.active[worker]);
        // Spent with the attempt, so a resumed entry starts unpaused. A cancel
        // outlives it — that ends the entry outright.
        slot.pause_requested.store(false, Ordering::Release);

        let settled = if slot.cancel_requested.load(Ordering::Acquire) {
            State::Cancelled
        } else {
            state
        };
        // Before the state, so a caller that sees a terminal state can read
        // the result behind it.
        if let (State::Complete, Some(digest)) = (settled, digest) {
            *lock(&slot.path) = Some(shared.transport.cache().path_for(digest));
        } else if let Some(failure) = failure {
            publish_failure(slot, failure);
        }
        if slot.state() != State::Preempted || settled != State::Paused {
            slot.set_state(settled);
        }
        released
    };
    // After the lock: dropping the last handle on a transfer closes files.
    drop(released);
    drop(download);
}

/// Ask the transfer in `index` to stop at its next poll, keeping its partial
/// file.
///
/// The flag goes down whether or not there is a handle to ask, so a worker
/// that has claimed the slot but not yet started the transport still sees it.
/// Call with the queue lock held.
fn request_pause(slot: &Slot, state: &WorkerState, index: usize) {
    slot.pause_requested.store(true, Ordering::Release);
    if let Some(download) = state.download_for(index) {
        download.request_pause();
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
    #[cfg_attr(not(test), allow(dead_code))]
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
        let workers = config.workers.clamp(1, MAX_WORKERS);
        Self {
            shared: Arc::new(Shared {
                transport,
                slots,
                queue_lock: Mutex::new(WorkerState::new(workers)),
                running: AtomicBool::new(true),
                workers,
                on_worker_start: config.on_worker_start,
            }),
        }
    }

    /// The transport this queue acquires through, and the cache it fills.
    #[must_use]
    pub fn transport(&self) -> &Transport {
        &self.shared.transport
    }

    /// Workers this queue drains with, and so transfers it runs at once.
    #[cfg_attr(not(test), allow(dead_code))]
    #[must_use]
    pub fn workers(&self) -> usize {
        self.shared.workers
    }

    /// Queue a transfer, or `None` when all [`QUEUE_MAX`] slots are in use.
    ///
    /// With every worker busy, a [`Priority::User`] request preempts one
    /// in-flight [`Priority::Background`] transfer, which pauses and
    /// auto-resumes once user-priority work has drained.
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
        slot.pause_requested.store(false, Ordering::Release);
        slot.bytes_downloaded.store(0, Ordering::Release);
        slot.bytes_total.store(UNKNOWN_TOTAL, Ordering::Release);
        slot.queue_time_ms.store(now_ms(), Ordering::Release);
        slot.set_state(State::Pending);
        slot.in_use.store(true, Ordering::Release);

        let mut snapshot = shared.snapshot_all();
        if let Some(target) = preempt_for_user(
            &mut snapshot,
            &state.claimed(),
            priority,
            state.has_idle_worker(),
        ) {
            shared.slots[target].apply(&snapshot[target]);
            request_pause(&shared.slots[target], &state, target);
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
        let state = shared.queue_lock();
        let mut snapshot = slot.snapshot();
        if !entry::cancel(&mut snapshot, state.is_active(id.0)) {
            return false;
        }
        slot.apply(&snapshot);
        // Gated on the claim, not the state: a preempted entry is still held.
        // The flag is what makes the promise stick — the transfer may be past
        // stopping, or have no handle yet, and either way its settle reads the
        // flag and reports Cancelled.
        if state.is_active(id.0) {
            slot.cancel_requested.store(true, Ordering::Release);
            if let Some(download) = state.download_for(id.0) {
                download.request_cancel();
            }
        }
        true
    }

    /// Cancel every transfer the queue holds, returning how many took it.
    pub fn cancel_all(&self) -> usize {
        (0..self.shared.slots.len())
            .filter(|&index| self.cancel(EntryId(index)))
            .count()
    }

    /// Pause an active transfer, keeping its partial file resumable. Returns
    /// whether the request applied.
    ///
    /// Unlike [`TransferQueue::cancel`], a pause that applies can still be
    /// overtaken: a transfer past its last read settles [`State::Complete`],
    /// having nothing left to pause.
    #[must_use]
    pub fn pause(&self, id: EntryId) -> bool {
        let shared = &self.shared;
        let state = shared.queue_lock();
        if !entry::pause(&shared.slots[id.0].snapshot(), state.is_active(id.0)) {
            return false;
        }
        request_pause(&shared.slots[id.0], &state, id.0);
        true
    }

    /// Pause every transfer in flight, returning how many took it.
    pub fn pause_all(&self) -> usize {
        (0..self.shared.slots.len())
            .filter(|&index| self.pause(EntryId(index)))
            .count()
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

    /// Resume every transfer a caller paused, returning how many took it.
    ///
    /// A preempted entry is not one of them: it resumes on its own once the
    /// user-priority work it yielded to has drained.
    pub fn resume_all(&self) -> usize {
        (0..self.shared.slots.len())
            .filter(|&index| self.resume(EntryId(index)))
            .count()
    }

    /// Free a slot. Refused while its transfer is active.
    #[must_use]
    pub fn remove(&self, id: EntryId) -> bool {
        let shared = &self.shared;
        let slot = &shared.slots[id.0];
        let state = shared.queue_lock();
        let mut snapshot = slot.snapshot();
        if !entry::remove(&mut snapshot, state.is_active(id.0)) {
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
    #[cfg_attr(not(test), allow(dead_code))]
    #[must_use]
    pub fn progress(&self, id: EntryId) -> f32 {
        self.shared.slots[id.0].snapshot().progress()
    }

    /// Whether any queued user-priority transfer is still waiting for a
    /// worker.
    #[cfg_attr(not(test), allow(dead_code))]
    #[must_use]
    pub fn has_user_pending(&self) -> bool {
        entry::has_user_pending(&self.shared.snapshot_all())
    }
}

impl Drop for TransferQueue {
    /// Cancel every active transfer, stop the workers re-arming, and block
    /// until all of them have exited.
    ///
    /// A cancel is observed between reads, so this waits out the reads in
    /// flight. The streaming agent bounds connecting but deliberately not the
    /// body — a large artifact on a slow link is not an error — so a peer that
    /// stops sending without closing holds the drop open.
    fn drop(&mut self) {
        self.shared.running.store(false, Ordering::Release);
        let handles = {
            let mut state = self.shared.queue_lock();
            for active in &state.active {
                if let Some(download) = active.download.as_ref() {
                    download.request_cancel();
                }
            }
            std::mem::take(&mut state.handles)
        };
        for handle in handles {
            let _ = handle.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicUsize;

    use super::*;

    fn queue_over(dir: &std::path::Path, config: Config) -> TransferQueue {
        TransferQueue::with_config(Transport::new(dir), config)
    }

    #[test]
    fn a_queue_drains_with_the_configured_worker_count() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(queue_over(dir.path(), Config::default()).workers(), 1);
        for requested in 1..=4 {
            let queue = queue_over(
                dir.path(),
                Config {
                    workers: requested,
                    ..Config::default()
                },
            );
            assert_eq!(queue.workers(), requested);
        }
    }

    #[test]
    fn a_worker_count_outside_the_range_is_pinned_to_it() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            queue_over(
                dir.path(),
                Config {
                    workers: 0,
                    ..Config::default()
                }
            )
            .workers(),
            1
        );
        assert_eq!(
            queue_over(
                dir.path(),
                Config {
                    workers: MAX_WORKERS + 7,
                    ..Config::default()
                }
            )
            .workers(),
            MAX_WORKERS
        );
    }

    /// The window the settle closes: `cancel` promised a stop against a
    /// transfer that then succeeded anyway.
    #[test]
    fn a_settle_reports_a_cancel_that_was_promised() {
        let dir = tempfile::tempdir().unwrap();
        let queue = queue_over(dir.path(), Config::default());
        let shared = &queue.shared;
        // No worker may run: this drives the settle by hand.
        shared.running.store(false, Ordering::Release);

        let id = queue.queue(request(0)).expect("a free slot");
        shared.queue_lock().active[0].slot = Some(id.0);
        shared.slots[id.0].set_state(State::Downloading);
        shared.slots[id.0]
            .cancel_requested
            .store(true, Ordering::Release);

        let digest = request(0).digest;
        finish(shared, 0, id.0, Some(&digest), None, State::Complete, None);

        assert_eq!(queue.state(id), State::Cancelled);
        assert_eq!(queue.path(id), None);
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
