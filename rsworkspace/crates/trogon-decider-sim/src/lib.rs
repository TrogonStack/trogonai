//! In-memory wasmtime host for Trogon decider WASM components.
#![cfg_attr(test, allow(clippy::expect_used, clippy::panic, clippy::unwrap_used))]

mod host;
mod import_check;
mod scenario;
mod session;

#[cfg(feature = "test-support")]
pub mod fixture;

#[cfg(feature = "test-support")]
pub use fixture::SimFixture;

pub use host::{SimError, SimHost, SimInstance};
pub use import_check::{ImportCheckError, assert_zero_imports};
pub use scenario::{ScenarioError, SimScenario};
pub use session::SimSession;
pub use trogon_decider_wasm_runtime::{WasmEngineConfig, WasmEngineError};
