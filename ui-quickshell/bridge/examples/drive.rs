//! drive <plugin-dir> <library-dir>: scans a directory, plays its first two rows, and walks
//! the transport through the C ABI the shell uses, printing what the state says after each
//! step. The way to check the engine's queue and transport without a window.
use std::ffi::{c_char, CStr, CString};
use std::thread::sleep;
use std::time::Duration;

use retrovert_ui_bridge::{
    rv_ui_clear_queue, rv_ui_document, rv_ui_free, rv_ui_new, rv_ui_next, rv_ui_play_rows,
    rv_ui_poll, rv_ui_previous, rv_ui_scan_library, rv_ui_seek, rv_ui_set_loop, rv_ui_set_volume,
    rv_ui_toggle_pause, rv_ui_vu, State,
};

fn state(s: *const std::ffi::c_void) -> State {
    let mut out = std::mem::MaybeUninit::<State>::zeroed();
    // SAFETY: s is live and out is writable.
    unsafe {
        rv_ui_poll(s, out.as_mut_ptr());
        out.assume_init()
    }
}

fn document(s: *const std::ffi::c_void, which: u32) -> String {
    let mut buf = vec![0 as c_char; 1 << 20];
    // SAFETY: s is live and buf holds 1 MiB.
    let n = unsafe { rv_ui_document(s, which, buf.as_mut_ptr(), 1 << 20) };
    // SAFETY: the bridge wrote a NUL-terminated string.
    let text = unsafe { CStr::from_ptr(buf.as_ptr()) }
        .to_string_lossy()
        .into_owned();
    format!("{text} ({n} bytes)")
}

fn field(doc: &str, key: &str) -> String {
    let needle = format!("\"{key}\":");
    doc.find(&needle)
        .map(|i| doc[i + needle.len()..].chars().take(40).collect())
        .unwrap_or_default()
}

fn report(s: *const std::ffi::c_void, step: &str) {
    let st = state(s);
    let mut vu = [0.0_f32; 8];
    // SAFETY: s is live and vu holds 8 floats.
    let n = unsafe { rv_ui_vu(s, vu.as_mut_ptr(), 8) };
    let track = document(s, 1);
    let queue = document(s, 2);
    println!(
        "{step}: status {} pos {} / {} sub {}/{} loop {} vol {:.2} ord {} row {} vu[{n}] {:?}\n   track {} row {}\n   queue {}",
        st.status,
        st.position_ms,
        st.duration_ms,
        st.subsong,
        st.subsong_count,
        st.loop_mode,
        st.volume,
        st.order,
        st.row,
        &vu[..n as usize],
        field(&track, "title"),
        field(&track, "row"),
        &queue[..queue.len().min(160)],
    );
}

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    let args: Vec<String> = std::env::args().collect();
    let plugins = CString::new(args[1].as_str()).expect("plugin dir");
    let library = CString::new(args[2].as_str()).expect("library dir");
    // SAFETY: both strings are NUL-terminated and live for the calls.
    let s = unsafe { rv_ui_new(plugins.as_ptr(), 1) };
    // SAFETY: s is live.
    unsafe { rv_ui_scan_library(s, library.as_ptr()) };
    for _ in 0..200 {
        let st = state(s);
        if st.library_total > 0 && st.library_scanned == st.library_total {
            break;
        }
        sleep(Duration::from_millis(50));
    }
    let st = state(s);
    println!(
        "library: {} of {} rows; {}",
        st.library_scanned,
        st.library_total,
        &document(s, 0)[..200]
    );

    let rows = [0_u32, 1, 0];
    // SAFETY: s is live and rows holds 3 ids.
    unsafe { rv_ui_play_rows(s, rows.as_ptr(), 3, 0) };
    sleep(Duration::from_millis(800));
    report(s, "play rows [0,1,0]");
    // SAFETY: s is live for every call below.
    unsafe {
        rv_ui_toggle_pause(s);
        sleep(Duration::from_millis(300));
        report(s, "paused");
        rv_ui_toggle_pause(s);
        rv_ui_seek(s, 20_000);
        sleep(Duration::from_millis(500));
        report(s, "resumed and sought to 20 s");
        rv_ui_previous(s);
        sleep(Duration::from_millis(500));
        report(s, "previous (restart, past 3 s)");
        rv_ui_next(s);
        sleep(Duration::from_millis(800));
        report(s, "next");
        rv_ui_previous(s);
        sleep(Duration::from_millis(800));
        report(s, "previous (early: back one)");
        rv_ui_set_loop(s, 2);
        rv_ui_set_volume(s, 0.25);
        sleep(Duration::from_millis(200));
        report(s, "loop all, volume 0.25");
        rv_ui_clear_queue(s);
        sleep(Duration::from_millis(200));
        report(s, "queue cleared");
        rv_ui_free(s);
    }
}
