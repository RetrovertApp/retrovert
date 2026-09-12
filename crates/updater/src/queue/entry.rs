//! The scheduling policy, modelled over plain values.
//!
//! Which pending entry runs next, how a cancel / pause / resume / remove
//! request resolves against an entry's state, and how preemption rewrites
//! states — all decided here, over owned [`Entry`] snapshots, so the policy is
//! exercised without a worker or a network.

use crate::policy;

/// `bytes_total` while the server has not said how long the transfer is.
pub(super) const UNKNOWN_TOTAL: i64 = -1;

/// Scheduling priority.
///
/// [`Priority::User`] outranks [`Priority::Background`]: a queued user transfer
/// preempts an active background one. The declaration order is the ranking
/// order.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[repr(i32)]
pub enum Priority {
    /// Work nobody is waiting on.
    Background = 0,
    /// Work someone asked for and is waiting on.
    User = 1,
}

impl Priority {
    /// The stored form, as held in an entry's shared atomic.
    pub(super) fn code(self) -> i32 {
        self as i32
    }

    /// Read back a stored priority. Anything unrecognised reads as
    /// [`Priority::Background`].
    pub(super) fn from_code(code: i32) -> Self {
        if code == Self::User.code() {
            Self::User
        } else {
            Self::Background
        }
    }
}

/// One entry's lifecycle state.
///
/// An entry's state is shared between the caller and the drain worker through
/// an `AtomicI32`, so the discriminants are explicit and [`State::from_code`]
/// is the only way back from the stored value.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(i32)]
pub enum State {
    /// Queued, waiting for a worker.
    Pending = 0,
    /// A worker is transferring it.
    Downloading = 1,
    /// Paused at the caller's request — stays paused until `resume`.
    Paused = 2,
    /// Paused by priority preemption — auto-resumes when the higher-priority
    /// work settles.
    Preempted = 3,
    /// Transferred and validated.
    Complete = 4,
    /// Stopped on an error.
    Failed = 5,
    /// Stopped at the caller's request.
    Cancelled = 6,
}

impl State {
    /// The stored form, as held in an entry's shared atomic.
    pub(super) fn code(self) -> i32 {
        self as i32
    }

    /// Read back a stored state. Only ever fed a value this enum wrote, so an
    /// unrecognised code is a bug rather than untrusted input; it reads as
    /// [`State::Pending`].
    pub(super) fn from_code(code: i32) -> Self {
        match code {
            1 => Self::Downloading,
            2 => Self::Paused,
            3 => Self::Preempted,
            4 => Self::Complete,
            5 => Self::Failed,
            6 => Self::Cancelled,
            _ => Self::Pending,
        }
    }
}

/// The scheduling view of one queued transfer: the fields the policy functions
/// read, snapshotted out of a slot's atomics.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) struct Entry {
    pub in_use: bool,
    pub priority: Priority,
    pub state: State,
    /// Wall-clock time the entry was queued; the tie-breaker for equal
    /// priority.
    pub queue_time_ms: i64,
    pub bytes_downloaded: i64,
    pub bytes_total: i64,
}

impl Entry {
    /// Fractional progress, or `0.0` when the total is unknown.
    pub(super) fn progress(&self) -> f32 {
        policy::progress(self.bytes_downloaded, self.bytes_total)
    }
}

/// Whether any in-use entry is a user-priority transfer still pending.
pub(super) fn has_user_pending(entries: &[Entry]) -> bool {
    entries
        .iter()
        .any(|e| e.in_use && e.priority == Priority::User && e.state == State::Pending)
}

/// Pick the next pending entry to run: highest priority first, ties broken by
/// earliest `queue_time_ms`. Returns its index, or `None` when nothing is
/// pending.
pub(super) fn next_pending(entries: &[Entry]) -> Option<usize> {
    let mut best: Option<usize> = None;
    for (i, e) in entries.iter().enumerate() {
        if !e.in_use || e.state != State::Pending {
            continue;
        }
        let take = match best {
            None => true,
            Some(b) => {
                e.priority > entries[b].priority
                    || (e.priority == entries[b].priority
                        && e.queue_time_ms < entries[b].queue_time_ms)
            }
        };
        if take {
            best = Some(i);
        }
    }
    best
}

// State transitions. Each returns whether the request applied. `is_active` is
// whether this entry is the one a drain worker currently owns.

/// Cancel an entry. One a worker still holds is asked to stop and keeps its
/// state for the settle to publish; anything else goes straight to Cancelled.
/// Any other state is a no-op.
///
/// A preempted entry can be either: the worker owns it until its pause
/// settles, and short-circuiting to Cancelled there loses the stop — the
/// settle would publish `Paused` over it.
pub(super) fn cancel(entry: &mut Entry, is_active: bool) -> bool {
    match entry.state {
        State::Downloading | State::Preempted if is_active => true,
        State::Pending | State::Paused | State::Preempted => {
            entry.state = State::Cancelled;
            true
        }
        _ => false,
    }
}

/// Pause an active transfer. Only a Downloading, currently-active entry can be
/// paused.
pub(super) fn pause(entry: &Entry, is_active: bool) -> bool {
    entry.state == State::Downloading && is_active
}

/// Resume a paused entry back to Pending. Only a Paused entry resumes.
pub(super) fn resume(entry: &mut Entry) -> bool {
    if entry.state == State::Paused {
        entry.state = State::Pending;
        true
    } else {
        false
    }
}

/// Free an entry's slot. Refused while a worker still holds it — freeing a
/// preempted entry would hand the slot to `queue` from under the transfer
/// still running in it.
pub(super) fn remove(entry: &mut Entry, is_active: bool) -> bool {
    if is_active || entry.state == State::Downloading {
        return false;
    }
    entry.in_use = false;
    true
}

/// Pick one active background transfer to yield its worker, mark it Preempted
/// so it pauses and later auto-resumes, and return its index.
///
/// Nothing yields while `idle_worker` — the queued entry runs on that worker
/// instead — or when the entry queued is itself background. Otherwise the
/// fewest bytes on disk goes, ties to whichever was queued last. Bytes rather
/// than the fraction covered, which is unknown until the server declares a
/// length.
///
/// `active` is the slots workers currently hold; indices outside `entries` are
/// ignored.
pub(super) fn preempt_for_user(
    entries: &mut [Entry],
    active: &[usize],
    queued_priority: Priority,
    idle_worker: bool,
) -> Option<usize> {
    if queued_priority != Priority::User || idle_worker {
        return None;
    }

    let mut best: Option<usize> = None;
    for &i in active {
        let Some(candidate) = entries.get(i) else {
            continue;
        };
        if !candidate.in_use
            || candidate.state != State::Downloading
            || candidate.priority != Priority::Background
        {
            continue;
        }
        let take = match best {
            None => true,
            Some(b) => {
                candidate.bytes_downloaded < entries[b].bytes_downloaded
                    || (candidate.bytes_downloaded == entries[b].bytes_downloaded
                        && candidate.queue_time_ms > entries[b].queue_time_ms)
            }
        };
        if take {
            best = Some(i);
        }
    }

    if let Some(i) = best {
        entries[i].state = State::Preempted;
    }
    best
}

/// Auto-resume every preempted entry back to Pending, once no user-priority
/// work is left to run.
///
/// The wait is on the queue rather than on the transfer that just settled:
/// with several workers another may still be on user work, and an entry
/// resumed under it would only be preempted again.
pub(super) fn auto_resume_preempted(entries: &mut [Entry]) {
    if entries.iter().any(is_user_work_outstanding) {
        return;
    }
    for e in entries.iter_mut() {
        if e.in_use && e.state == State::Preempted {
            e.state = State::Pending;
        }
    }
}

/// Whether an entry is user-priority work still queued or in flight. An
/// explicitly paused one is not: it waits on the caller, and background work
/// must not wait with it.
fn is_user_work_outstanding(entry: &Entry) -> bool {
    entry.in_use
        && entry.priority == Priority::User
        && matches!(entry.state, State::Pending | State::Downloading)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e(priority: Priority, state: State, queue_time_ms: i64) -> Entry {
        Entry {
            in_use: true,
            priority,
            state,
            queue_time_ms,
            bytes_downloaded: 0,
            bytes_total: UNKNOWN_TOTAL,
        }
    }

    #[test]
    fn progress_is_zero_until_the_total_is_known() {
        let mut entry = e(Priority::User, State::Pending, 0);
        assert!((entry.progress() - 0.0).abs() < 1e-6);
        entry.bytes_total = 100;
        entry.bytes_downloaded = 25;
        assert!((entry.progress() - 0.25).abs() < 1e-6);
    }

    #[test]
    fn next_pending_prefers_priority_then_time() {
        let entries = [
            e(Priority::Background, State::Pending, 10),
            e(Priority::User, State::Pending, 50),
            e(Priority::User, State::Pending, 20),
        ];
        // User at t=20 wins: higher priority than [0], earlier than [1].
        assert_eq!(next_pending(&entries), Some(2));
    }

    #[test]
    fn next_pending_skips_non_pending_and_free() {
        let mut entries = [
            e(Priority::User, State::Downloading, 10),
            e(Priority::Background, State::Pending, 20),
        ];
        entries[0].in_use = true;
        assert_eq!(next_pending(&entries), Some(1));

        let none = [e(Priority::User, State::Complete, 0)];
        assert_eq!(next_pending(&none), None);

        let mut freed = [e(Priority::User, State::Pending, 0)];
        freed[0].in_use = false;
        assert_eq!(next_pending(&freed), None);
    }

    #[test]
    fn has_user_pending_detects_user() {
        let entries = [
            e(Priority::Background, State::Pending, 0),
            e(Priority::User, State::Downloading, 0),
        ];
        assert!(!has_user_pending(&entries));
        let with_user = [e(Priority::User, State::Pending, 0)];
        assert!(has_user_pending(&with_user));
    }

    #[test]
    fn cancel_transitions() {
        let mut pending = e(Priority::User, State::Pending, 0);
        assert!(cancel(&mut pending, false));
        assert_eq!(pending.state, State::Cancelled);

        let mut paused = e(Priority::User, State::Paused, 0);
        assert!(cancel(&mut paused, false));
        assert_eq!(paused.state, State::Cancelled);

        // Preempted with its worker already gone: nothing left to stop.
        let mut preempted = e(Priority::Background, State::Preempted, 0);
        assert!(cancel(&mut preempted, false));
        assert_eq!(preempted.state, State::Cancelled);

        // Preempted but still held: handled, and the state is left for the
        // worker to settle. Going straight to Cancelled here would be
        // published over by the settle's `Paused`.
        let mut held = e(Priority::Background, State::Preempted, 0);
        assert!(cancel(&mut held, true));
        assert_eq!(held.state, State::Preempted);

        // Active downloading: handled, but the state is left for the worker to
        // settle.
        let mut active = e(Priority::User, State::Downloading, 0);
        assert!(cancel(&mut active, true));
        assert_eq!(active.state, State::Downloading);

        // Downloading but not the active entry: not handled.
        let mut inactive = e(Priority::User, State::Downloading, 0);
        assert!(!cancel(&mut inactive, false));

        // Already complete: no-op.
        let mut done = e(Priority::User, State::Complete, 0);
        assert!(!cancel(&mut done, false));
    }

    #[test]
    fn pause_takes_only_the_active_downloading_entry() {
        let downloading = e(Priority::User, State::Downloading, 0);
        assert!(pause(&downloading, true));
        assert!(!pause(&downloading, false));
        let pending = e(Priority::User, State::Pending, 0);
        assert!(!pause(&pending, true));
    }

    #[test]
    fn resume_takes_only_a_paused_entry() {
        let mut paused = e(Priority::User, State::Paused, 0);
        assert!(resume(&mut paused));
        assert_eq!(paused.state, State::Pending);
        let mut pending = e(Priority::User, State::Pending, 0);
        assert!(!resume(&mut pending));
        // A preempted entry resumes on its own sweep, not on request.
        let mut preempted = e(Priority::Background, State::Preempted, 0);
        assert!(!resume(&mut preempted));
    }

    #[test]
    fn remove_is_refused_while_a_worker_holds_the_slot() {
        let mut downloading = e(Priority::User, State::Downloading, 0);
        assert!(!remove(&mut downloading, true));
        assert!(downloading.in_use);

        // Preempted still means held: the worker settles it later, and freeing
        // the slot would let `queue` hand it out under the running transfer.
        let mut preempted = e(Priority::Background, State::Preempted, 0);
        assert!(!remove(&mut preempted, true));
        assert!(preempted.in_use);

        // Once the worker has let go, the same entry frees.
        assert!(remove(&mut preempted, false));
        assert!(!preempted.in_use);

        let mut complete = e(Priority::User, State::Complete, 0);
        assert!(remove(&mut complete, false));
        assert!(!complete.in_use);
    }

    #[test]
    fn a_user_transfer_preempts_one_active_background_one() {
        let mut entries = [
            e(Priority::Background, State::Downloading, 0),
            e(Priority::Background, State::Pending, 0),
        ];
        assert_eq!(
            preempt_for_user(&mut entries, &[0], Priority::User, false),
            Some(0)
        );
        assert_eq!(entries[0].state, State::Preempted);
        assert_eq!(entries[1].state, State::Pending);

        // A queued background transfer does not preempt.
        let mut background = [e(Priority::Background, State::Downloading, 0)];
        assert_eq!(
            preempt_for_user(&mut background, &[0], Priority::Background, false),
            None
        );
        assert_eq!(background[0].state, State::Downloading);

        // An active user transfer is not preempted by another user transfer.
        let mut active_user = [e(Priority::User, State::Downloading, 0)];
        assert_eq!(
            preempt_for_user(&mut active_user, &[0], Priority::User, false),
            None
        );
        assert_eq!(active_user[0].state, State::Downloading);

        // Nothing active is nothing to preempt.
        let mut idle = [e(Priority::Background, State::Pending, 0)];
        assert_eq!(
            preempt_for_user(&mut idle, &[], Priority::User, false),
            None
        );
    }

    /// Nothing yields while a worker is free: the queued user entry runs on
    /// that worker, so pausing background work would cost progress for
    /// nothing.
    #[test]
    fn a_free_worker_preempts_nothing() {
        let mut entries = [e(Priority::Background, State::Downloading, 0)];
        assert_eq!(
            preempt_for_user(&mut entries, &[0], Priority::User, true),
            None
        );
        assert_eq!(entries[0].state, State::Downloading);
    }

    /// With several workers busy on background work, the least-progressed
    /// transfer is the one that yields.
    #[test]
    fn the_least_progressed_background_transfer_yields() {
        let mut entries = [
            e(Priority::Background, State::Downloading, 10),
            e(Priority::Background, State::Downloading, 20),
            e(Priority::User, State::Downloading, 30),
        ];
        entries[0].bytes_downloaded = 900;
        entries[1].bytes_downloaded = 100;

        assert_eq!(
            preempt_for_user(&mut entries, &[0, 1, 2], Priority::User, false),
            Some(1)
        );
        assert_eq!(entries[1].state, State::Preempted);
        assert_eq!(entries[0].state, State::Downloading);
        assert_eq!(entries[2].state, State::Downloading);
    }

    /// Equal progress is a tie the entry queued last loses: the older transfer
    /// has been waiting longer to finish.
    #[test]
    fn equal_progress_yields_the_transfer_queued_last() {
        let mut entries = [
            e(Priority::Background, State::Downloading, 10),
            e(Priority::Background, State::Downloading, 50),
        ];
        assert_eq!(
            preempt_for_user(&mut entries, &[0, 1], Priority::User, false),
            Some(1)
        );
    }

    #[test]
    fn auto_resume_reactivates_only_preempted_entries() {
        let mut entries = [
            e(Priority::Background, State::Preempted, 0),
            e(Priority::User, State::Complete, 0),
            e(Priority::Background, State::Paused, 0),
        ];
        auto_resume_preempted(&mut entries);
        assert_eq!(entries[0].state, State::Pending);
        // Complete and explicitly-paused entries are untouched.
        assert_eq!(entries[1].state, State::Complete);
        assert_eq!(entries[2].state, State::Paused);
    }

    /// A preempted entry waits for the queue's user work to drain, not just
    /// for the transfer that displaced it — with several workers, another may
    /// still be on user work.
    #[test]
    fn auto_resume_waits_for_user_work_to_drain() {
        for holding in [State::Pending, State::Downloading] {
            let mut entries = [
                e(Priority::Background, State::Preempted, 0),
                e(Priority::User, holding, 0),
            ];
            auto_resume_preempted(&mut entries);
            assert_eq!(entries[0].state, State::Preempted, "held by {holding:?}");
        }

        // A user entry the caller paused holds nothing back: it waits on the
        // caller, not on a worker.
        let mut entries = [
            e(Priority::Background, State::Preempted, 0),
            e(Priority::User, State::Paused, 0),
        ];
        auto_resume_preempted(&mut entries);
        assert_eq!(entries[0].state, State::Pending);

        // Neither does a freed slot that still reads as user work.
        let mut freed = [
            e(Priority::Background, State::Preempted, 0),
            e(Priority::User, State::Pending, 0),
        ];
        freed[1].in_use = false;
        auto_resume_preempted(&mut freed);
        assert_eq!(freed[0].state, State::Pending);
    }

    #[test]
    fn a_state_round_trips_through_its_stored_code() {
        for state in [
            State::Pending,
            State::Downloading,
            State::Paused,
            State::Preempted,
            State::Complete,
            State::Failed,
            State::Cancelled,
        ] {
            assert_eq!(State::from_code(state.code()), state);
        }
        assert_eq!(State::from_code(99), State::Pending);

        for priority in [Priority::Background, Priority::User] {
            assert_eq!(Priority::from_code(priority.code()), priority);
        }
        assert_eq!(Priority::from_code(99), Priority::Background);
    }
}
