//! Writes `include/retrovert_ui.h` beside the crate so the C++ shim compiles against the
//! same signatures this build exported. Generation failing is a build failure, never a
//! stale header.

use std::path::PathBuf;

fn main() {
    let crate_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("cargo sets it"));
    let out = crate_dir.join("include").join("retrovert_ui.h");
    println!("cargo:rerun-if-changed=src");
    println!("cargo:rerun-if-changed=cbindgen.toml");
    cbindgen::generate(&crate_dir)
        .expect("cbindgen could not read the bridge crate")
        .write_to_file(&out);
}
