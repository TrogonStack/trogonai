//! Tier-3 WASM component bytes for the first-party pack.

/// Minimal WASM module stub for `schema-learner` (see `tests/fixtures/` and README).
pub const SCHEMA_LEARNER_STUB: &[u8] =
    include_bytes!("../../tests/fixtures/schema-learner-stub.wasm");

pub const SCHEMA_LEARNER_PATH: &str = "components/schema-learner.wasm";
pub const SCHEMA_LEARNER_ID: &str = "schema-learner";
