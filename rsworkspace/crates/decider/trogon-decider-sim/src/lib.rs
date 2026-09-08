//! In-memory wasmtime host for Trogon decider WASM components.
#![cfg_attr(test, allow(clippy::expect_used, clippy::panic, clippy::unwrap_used))]
#![cfg_attr(
    dylint_lib = "trogon_lints",
    expect(
        acyclic_modules,
        reason = "the host owns the sessions it instantiates and a session is typed with the host it runs against"
    )
)]

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
pub use ir::{
    BudgetOverrides, DomainErrorOutcome, ExpectedOutcome, ScenarioIr, ScenarioRun, ScenarioStep, StepOutcome,
    StreamIdOutcome, WireEnvelope,
};
#[cfg(feature = "test-support")]
pub use native::{
    NativeDecideError, NativeDeciderBundle, NativeDomainError, NativeRunError, decode_native_command, native_decide,
    native_evolve_one,
};
#[cfg(feature = "test-support")]
pub use parity::{ParityError, assert_parity};
pub use scenario::{GuestDomainError, ScenarioError, SimScenario};
pub use session::SimSession;
pub use trogon_decider_wasm_runtime::{ModuleName, ModuleNameError, WasmEngineConfig, WasmEngineError};
