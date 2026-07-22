//! Parsing for `decider-test` YAML suites, shared by the `decider-test` binary and by
//! `trogon-decider-sim`'s parity integration test.
//!
//! Keeping `Suite`/`Scenario`/`Step`/`Then` parsing (and their conversion into
//! [`trogon_decider_sim::ScenarioIr`]) in a library target means a test that wants to run every
//! scenario in a real YAML suite through both a native decider and a compiled wasm component can
//! reuse this exact parser, rather than re-implementing YAML-to-IR conversion a second time.
#![cfg_attr(test, allow(clippy::expect_used, clippy::panic, clippy::unwrap_used))]

pub mod codec;
mod suite;

pub use suite::{ErrorExpectation, Scenario, Step, Suite, Then};
