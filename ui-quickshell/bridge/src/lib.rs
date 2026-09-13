//! The C ABI the Quickshell shim calls.
//!
//! One opaque session owns the in-process audio worker in [`engine`] and answers the
//! render thread's per-frame questions from the worker's published front: how many scope
//! channels, what state, and the points of one channel into a buffer the caller owns.
//! That is the runtime contract's steady-state shape: caller-supplied capacity, refilled in
//! place, a count back.
//!
//! [`rv_ui_demo_tick`] remains for a session with nothing mounted, so the surfaces can be
//! seen moving without a song.

mod engine;

use std::ffi::{c_char, c_void, CStr};
use std::sync::Once;

use engine::{copy_points, lock, Engine, Status, TRACE_CAPACITY};
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
    /// Bumped once per captured frame.
    pub frame: u64,
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
    session.engine.open(std::path::PathBuf::from(path));
}

/// Drops the mounted song.
///
/// # Safety
/// `session` must be a live pointer from [`rv_ui_new`].
#[no_mangle]
pub unsafe extern "C" fn rv_ui_stop(session: *mut c_void) {
    // SAFETY: the caller promises a live session.
    let session = unsafe { &mut *session.cast::<Session>() };
    session.engine.stop();
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
        frame: front.frame,
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
            frame: 0,
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
        assert!(out.iter().any(|p| p.y != 0.0));
        let mut buf = [0 as c_char; 8];
        // SAFETY: s is live, buf has 8 bytes, and s is freed once.
        unsafe {
            assert_eq!(rv_ui_scope_points(s, 7, out.as_mut_ptr(), 8), 0);
            assert_eq!(rv_ui_error(s, buf.as_mut_ptr(), 8), 0);
            rv_ui_free(s);
            rv_ui_free(std::ptr::null_mut());
        }
    }
}
