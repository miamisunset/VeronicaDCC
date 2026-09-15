//! Build script: generate `target/include/veronica.h` via cbindgen.

// Build scripts run at compile time; a panic here is a build failure with
// a message, not a runtime crash — unwraps are the idiomatic choice.
#![allow(
    clippy::unwrap_used,
    reason = "build script: panic surfaces as a build error"
)]

extern crate cbindgen;

use std::env;

fn main() {
    let crate_dir = env::var("CARGO_MANIFEST_DIR").unwrap();
    let bindings = cbindgen::generate(&crate_dir).unwrap_or_else(|err| {
        eprintln!("cbindgen failed: {err}");
        std::process::exit(1);
    });
    let out_path = format!("{crate_dir}/../../target/include/veronica.h");
    std::fs::create_dir_all(format!("{crate_dir}/../../target/include")).unwrap();
    bindings.write_to_file(&out_path);
    println!("cargo:rerun-if-changed=src/lib.rs");
    println!("cargo:rerun-if-changed=cbindgen.toml");
}
