//! Checked-in WASM fixture loader shared by this crate's own unit tests.
//!
//! Integration tests under `tests/` load the same artifact independently
//! (they cannot see crate-private items), following the same convention as
//! `trogon-decider-sim/src/fixture.rs`.

use std::fs;
use std::path::Path;

#[allow(
    clippy::panic,
    reason = "test fixture loader; a missing pre-built wasm artifact is an unrecoverable test setup error"
)]
pub(crate) fn schedules_bytes() -> Vec<u8> {
    let relative = "../../target/wasm32-unknown-unknown/release/trogon_schedules_decider.wasm";
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(relative);
    fs::read(&path).unwrap_or_else(|error| {
        panic!(
            "build trogon_schedules_decider.wasm for wasm32-unknown-unknown first (expected {}): {error}",
            path.display()
        )
    })
}
