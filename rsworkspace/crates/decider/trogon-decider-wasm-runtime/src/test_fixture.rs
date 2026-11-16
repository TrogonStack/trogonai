//! Checked-in WASM fixture loader shared by this crate's own unit tests.
//!
//! Integration tests under `tests/` load the same artifact independently
//! (they cannot see crate-private items), following the same convention as
//! `trogon-decider-sim/src/fixture.rs`.

use std::fs;
use std::path::Path;
use std::sync::OnceLock;

use crate::{WasmDeciderEngine, WasmDeciderModule, WasmEngineConfig};

pub(crate) fn schedules_bytes() -> Vec<u8> {
    let relative = "../../../target/wasm32-unknown-unknown/release/trogon_schedules_decider.wasm";
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(relative);
    let missing_fixture = format!(
        "build trogon_schedules_decider.wasm for wasm32-unknown-unknown first (expected {})",
        path.display()
    );
    fs::read(&path).expect(&missing_fixture)
}

/// The fixture compiled once for the whole test process.
///
/// Compiling the component costs more than everything else a test here does,
/// and Cranelift runs unoptimized in debug and coverage builds, so compiling it
/// per test multiplies that cost by the number of tests. A clone shares the
/// engine and the pre-instantiated component, the way production shares one
/// engine per process; every session still gets its own store and guest
/// instance.
pub(crate) fn schedules_module() -> WasmDeciderModule {
    static MODULE: OnceLock<WasmDeciderModule> = OnceLock::new();
    MODULE
        .get_or_init(|| {
            let engine = WasmDeciderEngine::new(WasmEngineConfig::default()).expect("engine builds");
            WasmDeciderModule::load(engine, &schedules_bytes()).expect("module loads")
        })
        .clone()
}
