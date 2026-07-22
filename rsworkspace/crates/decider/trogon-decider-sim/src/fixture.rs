//! Checked-in WASM fixtures for integration tests.

use std::fs;
use std::path::{Path, PathBuf};

/// A compiled decider component used by sim integration tests.
pub struct SimFixture {
    bytes: Vec<u8>,
}

impl SimFixture {
    /// Loads the pre-built `trogon_schedules_decider.wasm` release artifact from the workspace
    /// target directory.
    pub fn schedules() -> Self {
        Self::load(
            "trogon_schedules_decider.wasm",
            "../../../target/wasm32-unknown-unknown/release/trogon_schedules_decider.wasm",
        )
    }

    /// Returns the fixture's compiled component bytes.
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    #[allow(
        clippy::panic,
        reason = "test fixture loader; a missing pre-built wasm artifact is an unrecoverable test setup error"
    )]
    fn load(name: &str, relative: &str) -> Self {
        let path = fixture_path(relative);
        let bytes = fs::read(&path).unwrap_or_else(|error| {
            panic!(
                "build {name} for wasm32-unknown-unknown first (expected {}): {error}",
                path.display()
            )
        });
        Self { bytes }
    }
}

fn fixture_path(relative: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(relative)
}
