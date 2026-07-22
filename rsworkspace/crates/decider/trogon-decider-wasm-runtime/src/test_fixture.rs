//! Checked-in WASM fixture loader shared by this crate's own unit tests.
//!
//! Integration tests under `tests/` load the same artifact independently
//! (they cannot see crate-private items), following the same convention as
//! `trogon-decider-sim/src/fixture.rs`.

use std::fs;
use std::path::Path;

pub(crate) fn schedules_bytes() -> Vec<u8> {
    let relative = "../../../target/wasm32-unknown-unknown/release/trogon_schedules_decider.wasm";
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(relative);
    let missing_fixture = format!(
        "build trogon_schedules_decider.wasm for wasm32-unknown-unknown first (expected {})",
        path.display()
    );
    fs::read(&path).expect(&missing_fixture)
}
