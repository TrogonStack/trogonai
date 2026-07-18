//! In-memory wasmtime host for Trogon decider WASM components.
#![cfg_attr(test, allow(clippy::expect_used, clippy::panic, clippy::unwrap_used))]

mod host;
mod import_check;
pub mod ir;
mod scenario;
mod session;

#[cfg(feature = "test-support")]
pub mod fixture;

#[cfg(feature = "test-support")]
pub mod native;

#[cfg(feature = "test-support")]
pub mod parity;

#[cfg(feature = "test-support")]
pub use fixture::SimFixture;

pub use host::{SimError, SimHost, SimInstance};
pub use import_check::{ImportCheckError, assert_zero_imports};
pub use ir::{ExpectedOutcome, ScenarioIr, ScenarioStep, StepOutcome, WireEnvelope};
#[cfg(feature = "test-support")]
pub use native::{
    NativeDecideError, NativeDeciderBundle, NativeDomainError, NativeRunError, decode_native_command, native_decide,
    native_evolve_one,
};
#[cfg(feature = "test-support")]
pub use parity::{ParityError, assert_parity};
pub use scenario::{ScenarioError, SimScenario};
pub use session::SimSession;
pub use trogon_decider_wasm_runtime::{WasmEngineConfig, WasmEngineError};
