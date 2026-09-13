//! The C ABI the Quickshell shim calls.
//!
//! One opaque session owns the in-process audio worker in [`engine`] and answers the
//! render thread's per-frame questions from the worker's published front: how many scope
//! channels, what state, and the points of one channel into a buffer the caller owns.
//! That is the runtime contract's steady-state shape: caller-supplied capacity, refilled in
//! place, a count back.
//!
//! What is not per frame, the library, the playing song's metadata and the queue, is read as
//! JSON through [`rv_ui_document`] whenever the revision counters in [`State`] move, so the
//! C surface stays small while those records grow.
//!
//! [`rv_ui_demo_tick`] remains for a session with nothing mounted, so the surfaces can be
//! seen moving without a song.

mod engine;

use std::ffi::{c_char, c_void, CStr};
use std::sync::Once;

use engine::{copy_points, lock, Command, Document, Engine, LoopMode, Status, TRACE_CAPACITY};
use retrovert_present::{ScopePoint, ScopeTrace};

/// One UI session: the engine plus the demo traces used while nothing is mounted.
pub struct Session {
    engine: Engine,
    demo_traces: Vec<ScopeTrace>,
    demo: Vec<f32>,
}

/// What [`rv_ui_poll`] reports, copied out under the front lock.
#[repr(C)]
pub struct State {
    /// Scope channels of the mounted song, or the demo's while idle.
    pub channels: u32,
    /// One of [`Status`], as its discriminant.
    pub status: u32,
    /// Delivered position in milliseconds.
    pub position_ms: i64,
    /// Length of the playing subsong in milliseconds, 0 when unknown.
    pub duration_ms: i64,
    /// Bumped once per captured frame.
    pub frame: u64,
    /// The playing subsong, and how many the song holds.
    pub subsong: u32,
    /// See `subsong`.
    pub subsong_count: u32,
    /// 1 when the decoder reports a tracker position in the three fields after it.
    pub has_position: u32,
    /// Order-list index.
    pub order: u32,
    /// Pattern number.
    pub pattern: u32,
    /// Row within the pattern.
    pub row: u32,
    /// Output sample rate in Hz.
    pub sample_rate: u32,
    /// One of [`LoopMode`], as its discriminant.
    pub loop_mode: u32,
    /// Output gain, 0..=1.
    pub volume: f32,
    /// Library rows described so far, and how many the walk found.
    pub library_scanned: u32,
    /// See `library_scanned`.
    pub library_total: u32,
    /// Bumped whenever the library document changes.
    pub library_rev: u64,
    /// Bumped whenever the track document changes.
    pub track_rev: u64,
    /// Bumped whenever the queue document changes.
    pub queue_rev: u64,
}

impl Session {
    fn new(plugin_dir: &str, demo_channels: usize) -> Self {
        Self {
            engine: Engine::start(std::path::Path::new(plugin_dir)),
            demo_traces: (0..demo_channels)
                .map(|_| ScopeTrace::new(TRACE_CAPACITY))
                .collect(),
            demo: vec![0.0; 512],
        }
    }

    fn idle(&self) -> bool {
        lock(&self.engine.front).status == Status::Idle
    }

    /// Fills every demo trace with a phase-shifted sine so each channel looks distinct.
    #[allow(clippy::cast_precision_loss)]
    fn demo_tick(&mut self, seconds: f64) {
        let n = self.demo.len();
        for (channel, trace) in self.demo_traces.iter_mut().enumerate() {
            let freq = 2.0 + channel as f64;
            for (i, sample) in self.demo.iter_mut().enumerate() {
                let t = i as f64 / n as f64;
                let phase = std::f64::consts::TAU * (t * freq + seconds * 0.5);
                #[allow(clippy::cast_possible_truncation)]
                let value = (phase.sin() * (0.9 - 0.15 * channel as f64)) as f32;
                *sample = value;
            }
            trace.fill(&self.demo);
        }
    }
}

fn init_logging() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let _ = env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
            .try_init();
    });
}

unsafe fn text<'a>(ptr: *const c_char) -> &'a str {
    if ptr.is_null() {
        return "";
    }
    // SAFETY: the caller promises a NUL-terminated string.
    unsafe { CStr::from_ptr(ptr) }.to_str().unwrap_or("")
}

/// Copies `s` into `out` as a NUL-terminated string, truncating to `capacity`.
unsafe fn write_text(s: &str, out: *mut c_char, capacity: u32) -> u32 {
    let capacity = usize::try_from(capacity).unwrap_or(0);
    if out.is_null() || capacity == 0 {
        return 0;
    }
    let bytes = s.as_bytes();
    let n = bytes.len().min(capacity - 1);
    // SAFETY: the caller promises `capacity` writable bytes at `out`; n + 1 <= capacity.
    unsafe {
        std::ptr::copy_nonoverlapping(bytes.as_ptr().cast::<c_char>(), out, n);
        *out.add(n) = 0;
    }
    u32::try_from(n).unwrap_or(u32::MAX)
}

/// Creates a session whose worker loads every plugin under `plugin_dir`, with
/// `demo_channels` synthetic traces shown while nothing is mounted. Free with [`rv_ui_free`].
///
/// # Safety
/// `plugin_dir` must be null or a NUL-terminated string.
#[no_mangle]
pub unsafe extern "C" fn rv_ui_new(plugin_dir: *const c_char, demo_channels: u32) -> *mut c_void {
    init_logging();
    // SAFETY: the caller promises a NUL-terminated string or null.
    let dir = unsafe { text(plugin_dir) };
    let demo_channels = usize::try_from(demo_channels).unwrap_or(0).clamp(1, 64);
    Box::into_raw(Box::new(Session::new(dir, demo_channels))).cast()
}

/// Destroys a session made by [`rv_ui_new`], stopping its worker. A null pointer is ignored.
///
/// # Safety
/// `session` must be null or a pointer from [`rv_ui_new`] not yet freed.
#[no_mangle]
pub unsafe extern "C" fn rv_ui_free(session: *mut c_void) {
    if session.is_null() {
        return;
    }
    // SAFETY: the caller promises this came from rv_ui_new and is freed once.
    drop(unsafe { Box::from_raw(session.cast::<Session>()) });
}

/// Asks the worker to mount `path`. The outcome arrives through [`rv_ui_poll`].
///
/// # Safety
/// `session` must be a live pointer from [`rv_ui_new`]; `path` a NUL-terminated string.
#[no_mangle]
pub unsafe extern "C" fn rv_ui_open(session: *mut c_void, path: *const c_char) {
    // SAFETY: the caller promises a live session and a NUL-terminated string.
    let (session, path) = unsafe { (&mut *session.cast::<Session>(), text(path)) };
    session
        .engine
        .send(Command::Open(std::path::PathBuf::from(path)));
}

/// Plays library rows `rows[0..count]` in that order, starting at `rows[start]`. The rows
/// are the ids of the library document; unknown ids are skipped.
///
/// # Safety
/// `session` must be a live pointer from [`rv_ui_new`]; `rows` must point to `count`
/// readable `u32`s, or be null with `count` 0.
#[no_mangle]
pub unsafe extern "C" fn rv_ui_play_rows(
    session: *mut c_void,
    rows: *const u32,
    count: u32,
    start: u32,
) {
    // SAFETY: the caller promises a live session.
    let session = unsafe { &mut *session.cast::<Session>() };
    let count = usize::try_from(count).unwrap_or(0);
    if rows.is_null() || count == 0 {
        return;
    }
    // SAFETY: the caller promises `count` readable u32s at `rows`.
    let rows = unsafe { std::slice::from_raw_parts(rows, count) }.to_vec();
    let start = usize::try_from(start).unwrap_or(0);
    session.engine.send(Command::PlayRows(rows, start));
}

/// Holds or resumes the mounted song.
///
/// # Safety
/// `session` must be a live pointer from [`rv_ui_new`].
#[no_mangle]
pub unsafe extern "C" fn rv_ui_toggle_pause(session: *mut c_void) {
    // SAFETY: the caller promises a live session.
    unsafe { &mut *session.cast::<Session>() }
        .engine
        .send(Command::TogglePause);
}

/// Moves the playhead to `position_ms`, when the decoder can.
///
/// # Safety
/// `session` must be a live pointer from [`rv_ui_new`].
#[no_mangle]
pub unsafe extern "C" fn rv_ui_seek(session: *mut c_void, position_ms: i64) {
    // SAFETY: the caller promises a live session.
    unsafe { &mut *session.cast::<Session>() }
        .engine
        .send(Command::Seek(position_ms));
}

/// Skips to the next queued song.
///
/// # Safety
/// `session` must be a live pointer from [`rv_ui_new`].
#[no_mangle]
pub unsafe extern "C" fn rv_ui_next(session: *mut c_void) {
    // SAFETY: the caller promises a live session.
    unsafe { &mut *session.cast::<Session>() }
        .engine
        .send(Command::Next);
}

/// Restarts the song, or within its first seconds goes back to the one before.
///
/// # Safety
/// `session` must be a live pointer from [`rv_ui_new`].
#[no_mangle]
pub unsafe extern "C" fn rv_ui_previous(session: *mut c_void) {
    // SAFETY: the caller promises a live session.
    unsafe { &mut *session.cast::<Session>() }
        .engine
        .send(Command::Previous);
}

/// Opens subsong `index` of the playing song; an index past the count is ignored.
///
/// # Safety
/// `session` must be a live pointer from [`rv_ui_new`].
#[no_mangle]
pub unsafe extern "C" fn rv_ui_set_subsong(session: *mut c_void, index: u32) {
    // SAFETY: the caller promises a live session.
    unsafe { &mut *session.cast::<Session>() }
        .engine
        .send(Command::SetSubsong(index));
}

/// Sets what happens when a song ends; an unknown value reads as [`LoopMode::Off`].
///
/// # Safety
/// `session` must be a live pointer from [`rv_ui_new`].
#[no_mangle]
pub unsafe extern "C" fn rv_ui_set_loop(session: *mut c_void, mode: u32) {
    // SAFETY: the caller promises a live session.
    unsafe { &mut *session.cast::<Session>() }
        .engine
        .send(Command::SetLoop(LoopMode::from_u32(mode)));
}

/// Sets the output gain, clamped to 0..=1.
///
/// # Safety
/// `session` must be a live pointer from [`rv_ui_new`].
#[no_mangle]
pub unsafe extern "C" fn rv_ui_set_volume(session: *mut c_void, volume: f32) {
    // SAFETY: the caller promises a live session.
    unsafe { &mut *session.cast::<Session>() }
        .engine
        .send(Command::SetVolume(volume));
}

/// Keeps the playing song and drops everything queued after it.
///
/// # Safety
/// `session` must be a live pointer from [`rv_ui_new`].
#[no_mangle]
pub unsafe extern "C" fn rv_ui_clear_queue(session: *mut c_void) {
    // SAFETY: the caller promises a live session.
    unsafe { &mut *session.cast::<Session>() }
        .engine
        .send(Command::ClearQueue);
}

/// Walks `root` and describes every file a decoder claims; rows arrive through the library
/// document as they are described.
///
/// # Safety
/// `session` must be a live pointer from [`rv_ui_new`]; `root` a NUL-terminated string.
#[no_mangle]
pub unsafe extern "C" fn rv_ui_scan_library(session: *mut c_void, root: *const c_char) {
    // SAFETY: the caller promises a live session and a NUL-terminated string.
    let (session, root) = unsafe { (&mut *session.cast::<Session>(), text(root)) };
    session
        .engine
        .send(Command::ScanLibrary(std::path::PathBuf::from(root)));
}

/// Copies document `which` into `out` as NUL-terminated UTF-8 and returns the document's
/// full length in bytes, which may exceed `capacity - 1`: the caller then grows its buffer
/// and asks again. An unknown document returns 0 and writes an empty string.
///
/// # Safety
/// `session` must be a live pointer from [`rv_ui_new`]; `out` must hold `capacity` bytes.
#[no_mangle]
pub unsafe extern "C" fn rv_ui_document(
    session: *const c_void,
    which: u32,
    out: *mut c_char,
    capacity: u32,
) -> u32 {
    // SAFETY: the caller promises a live session.
    let session = unsafe { &*session.cast::<Session>() };
    let Some(which) = Document::from_u32(which) else {
        // SAFETY: the caller promises `capacity` bytes at `out`.
        unsafe { write_text("", out, capacity) };
        return 0;
    };
    let docs = lock(&session.engine.docs);
    let doc = docs.get(which);
    // SAFETY: the caller promises `capacity` bytes at `out`.
    unsafe { write_text(doc, out, capacity) };
    u32::try_from(doc.len()).unwrap_or(u32::MAX)
}

/// Copies the decayed VU levels, one per scope channel in 0..=1, into `out`, at most
/// `capacity` of them, and returns how many were written.
///
/// # Safety
/// `session` must be a live pointer from [`rv_ui_new`]; `out` must point to `capacity`
/// writable floats, or be null with `capacity` 0.
#[no_mangle]
pub unsafe extern "C" fn rv_ui_vu(session: *const c_void, out: *mut f32, capacity: u32) -> u32 {
    // SAFETY: the caller promises a live session.
    let session = unsafe { &*session.cast::<Session>() };
    let capacity = usize::try_from(capacity).unwrap_or(0);
    if out.is_null() || capacity == 0 {
        return 0;
    }
    // SAFETY: the caller promises `capacity` writable floats at `out`.
    let dst = unsafe { std::slice::from_raw_parts_mut(out, capacity) };
    let front = lock(&session.engine.front);
    let n = front.vu_len.min(capacity);
    dst[..n].copy_from_slice(&front.vu[..n]);
    u32::try_from(n).unwrap_or(u32::MAX)
}

/// Drops the mounted song.
///
/// # Safety
/// `session` must be a live pointer from [`rv_ui_new`].
#[no_mangle]
pub unsafe extern "C" fn rv_ui_stop(session: *mut c_void) {
    // SAFETY: the caller promises a live session.
    let session = unsafe { &mut *session.cast::<Session>() };
    session.engine.send(Command::Stop);
}

/// Reports the current state into `out`.
///
/// # Safety
/// `session` must be a live pointer from [`rv_ui_new`]; `out` must be writable.
#[no_mangle]
pub unsafe extern "C" fn rv_ui_poll(session: *const c_void, out: *mut State) {
    // SAFETY: the caller promises a live session and a writable State.
    let (session, out) = unsafe { (&*session.cast::<Session>(), &mut *out) };
    let front = lock(&session.engine.front);
    let channels = if front.status == Status::Idle {
        session.demo_traces.len()
    } else {
        front.traces.len()
    };
    *out = State {
        channels: u32::try_from(channels).unwrap_or(u32::MAX),
        status: front.status as u32,
        position_ms: front.position_ms,
        duration_ms: front.duration_ms,
        frame: front.frame,
        subsong: front.subsong,
        subsong_count: front.subsong_count,
        has_position: u32::from(front.position.has),
        order: front.position.order,
        pattern: front.position.pattern,
        row: front.position.row,
        sample_rate: engine::SAMPLE_RATE,
        loop_mode: front.loop_mode as u32,
        volume: front.volume,
        library_scanned: front.library_scanned,
        library_total: front.library_total,
        library_rev: front.library_rev,
        track_rev: front.track_rev,
        queue_rev: front.queue_rev,
    };
}

/// Copies the decoder name into `out`, NUL-terminated; returns the length written.
///
/// # Safety
/// `session` must be a live pointer from [`rv_ui_new`]; `out` must hold `capacity` bytes.
#[no_mangle]
pub unsafe extern "C" fn rv_ui_plugin_name(
    session: *const c_void,
    out: *mut c_char,
    capacity: u32,
) -> u32 {
    // SAFETY: the caller promises a live session.
    let session = unsafe { &*session.cast::<Session>() };
    let plugin = lock(&session.engine.front).plugin.clone();
    // SAFETY: the caller promises `capacity` bytes at `out`.
    unsafe { write_text(&plugin, out, capacity) }
}

/// Copies the last error into `out`, NUL-terminated; returns the length written, 0 if none.
///
/// # Safety
/// `session` must be a live pointer from [`rv_ui_new`]; `out` must hold `capacity` bytes.
#[no_mangle]
pub unsafe extern "C" fn rv_ui_error(
    session: *const c_void,
    out: *mut c_char,
    capacity: u32,
) -> u32 {
    // SAFETY: the caller promises a live session.
    let session = unsafe { &*session.cast::<Session>() };
    let error = lock(&session.engine.front).error.clone();
    // SAFETY: the caller promises `capacity` bytes at `out`.
    unsafe { write_text(&error, out, capacity) }
}

/// Advances the synthetic waveform to `seconds`. Does nothing while a song is mounted.
///
/// # Safety
/// `session` must be a live pointer from [`rv_ui_new`].
#[no_mangle]
pub unsafe extern "C" fn rv_ui_demo_tick(session: *mut c_void, seconds: f64) {
    // SAFETY: the caller promises a live session and no concurrent access.
    let session = unsafe { &mut *session.cast::<Session>() };
    if session.idle() {
        session.demo_tick(seconds);
    }
}

/// Copies channel `channel`'s polyline into `out`, at most `capacity` points, and returns
/// how many were written. An unknown channel writes nothing and returns 0.
///
/// # Safety
/// `session` must be a live pointer from [`rv_ui_new`]; `out` must point to at least
/// `capacity` writable [`ScopePoint`]s, or be null with `capacity` 0.
#[no_mangle]
pub unsafe extern "C" fn rv_ui_scope_points(
    session: *const c_void,
    channel: u32,
    out: *mut ScopePoint,
    capacity: u32,
) -> u32 {
    // SAFETY: the caller promises a live session.
    let session = unsafe { &*session.cast::<Session>() };
    let capacity = usize::try_from(capacity).unwrap_or(0);
    let Ok(channel) = usize::try_from(channel) else {
        return 0;
    };
    if out.is_null() || capacity == 0 {
        return 0;
    }
    // SAFETY: the caller promises `capacity` writable points at `out`.
    let dst = unsafe { std::slice::from_raw_parts_mut(out, capacity) };
    let front = lock(&session.engine.front);
    let n = if front.status == Status::Idle {
        session.demo_traces.get(channel).map_or(0, |trace| {
            let src = trace.points();
            let n = src.len().min(capacity);
            dst[..n].copy_from_slice(&src[..n]);
            n
        })
    } else {
        copy_points(&front, channel, dst)
    };
    u32::try_from(n).unwrap_or(u32::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn demo_round_trip_through_the_abi() {
        let dir = std::ffi::CString::new("/nonexistent").unwrap_or_default();
        // SAFETY: dir is NUL-terminated.
        let s = unsafe { rv_ui_new(dir.as_ptr(), 2) };
        let mut out = [ScopePoint::default(); 8];
        let mut state = State {
            channels: 0,
            status: 9,
            position_ms: 0,
            duration_ms: 0,
            frame: 0,
            subsong: 0,
            subsong_count: 0,
            has_position: 0,
            order: 0,
            pattern: 0,
            row: 0,
            sample_rate: 0,
            loop_mode: 0,
            volume: 0.0,
            library_scanned: 0,
            library_total: 0,
            library_rev: 0,
            track_rev: 0,
            queue_rev: 0,
        };
        // SAFETY: s is live, out has 8 points, state is writable.
        let n = unsafe {
            rv_ui_demo_tick(s, 0.25);
            rv_ui_poll(s, &raw mut state);
            rv_ui_scope_points(s, 1, out.as_mut_ptr(), 8)
        };
        assert_eq!(n, 8);
        assert_eq!(state.channels, 2);
        assert_eq!(state.status, Status::Idle as u32);
        assert_eq!(state.sample_rate, engine::SAMPLE_RATE);
        assert!(out.iter().any(|p| p.y != 0.0));
        let mut buf = [0 as c_char; 8];
        let mut levels = [0.0_f32; 4];
        // SAFETY: s is live, buf has 8 bytes, levels has 4 floats, and s is freed once.
        unsafe {
            assert_eq!(rv_ui_scope_points(s, 7, out.as_mut_ptr(), 8), 0);
            assert_eq!(rv_ui_error(s, buf.as_mut_ptr(), 8), 0);
            assert_eq!(rv_ui_vu(s, levels.as_mut_ptr(), 4), 0);
            rv_ui_free(s);
            rv_ui_free(std::ptr::null_mut());
        }
    }

    #[test]
    fn documents_report_their_full_length_and_truncate_to_the_buffer() {
        let dir = std::ffi::CString::new("/nonexistent").unwrap_or_default();
        // SAFETY: dir is NUL-terminated.
        let s = unsafe { rv_ui_new(dir.as_ptr(), 1) };
        // The worker publishes the empty library on start; wait for that revision.
        let mut state = State {
            channels: 0,
            status: 0,
            position_ms: 0,
            duration_ms: 0,
            frame: 0,
            subsong: 0,
            subsong_count: 0,
            has_position: 0,
            order: 0,
            pattern: 0,
            row: 0,
            sample_rate: 0,
            loop_mode: 0,
            volume: 0.0,
            library_scanned: 0,
            library_total: 0,
            library_rev: 0,
            track_rev: 0,
            queue_rev: 0,
        };
        for _ in 0..500 {
            // SAFETY: s is live and state is writable.
            unsafe { rv_ui_poll(s, &raw mut state) };
            if state.library_rev > 0 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(
            state.library_rev > 0,
            "the worker never published a library"
        );
        let mut small = [0 as c_char; 4];
        // SAFETY: s is live and small has 4 bytes.
        let full = unsafe { rv_ui_document(s, Document::Library as u32, small.as_mut_ptr(), 4) };
        assert!(full > 3, "{full}");
        assert_eq!(small[3], 0);
        let mut big = vec![0 as c_char; full as usize + 1];
        // SAFETY: s is live and big has full + 1 bytes.
        let again =
            unsafe { rv_ui_document(s, Document::Library as u32, big.as_mut_ptr(), full + 1) };
        assert_eq!(again, full);
        // SAFETY: big holds a NUL-terminated string the bridge just wrote.
        let json = unsafe { CStr::from_ptr(big.as_ptr()) }
            .to_str()
            .unwrap_or("");
        assert!(json.starts_with("{\"root\":"), "{json}");
        // SAFETY: s is live and freed once.
        unsafe {
            assert_eq!(rv_ui_document(s, 99, small.as_mut_ptr(), 4), 0);
            rv_ui_free(s);
        }
    }
}
