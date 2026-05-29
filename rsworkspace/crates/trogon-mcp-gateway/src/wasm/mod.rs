//! Wasmtime policy bundle surface (Block F item 2).

mod bindings;
mod config;
mod engine;
mod error;
mod host_impl;
mod pool;
mod runtime;
mod store_state;
mod wasi_stub;

pub use bindings::{
    contract_identity, ContractIdentity, HostFailure, HostImports, LogLevel, PolicyDecision,
    PolicyGuest, RequestCtx, SpanContext, SpicedbBinding, ToolDescriptor, UnlinkedHost,
    WIT_PACKAGE, WIT_VERSION, WORLD_NAME,
};
pub use config::PoolConfig;
pub use engine::{WasmBundleHandle, WasmEngine};
pub use error::{EvaluateOutcome, WasmEngineError, WasmFaultCode};
pub use pool::{ComponentPool, PoolKey};
pub use store_state::WasmStoreState;

/// Returns an unlinked host stub and validates contract pins at call sites.
#[must_use]
pub fn stub_host() -> UnlinkedHost {
    let _ = contract_identity();
    UnlinkedHost
}
