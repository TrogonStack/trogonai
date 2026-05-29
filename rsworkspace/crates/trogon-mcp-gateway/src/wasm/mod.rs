//! Wasmtime policy bundle surface (Block F item 2).

mod bindings;
mod config;
mod engine;
mod error;
mod host_impl;
mod pool;
mod runtime;
mod store_state;
mod tracing;
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
pub use tracing::{
    extract_trace_id, extract_traceparent, parent_from_span_context, populate_request_span,
    span_context_from_current, traceparent_is_sampled, tracing_level_from_log,
    ParsedTraceParent, WASM_EVALUATE_SPAN_NAME,
};

/// Returns an unlinked host stub and validates contract pins at call sites.
#[must_use]
pub fn stub_host() -> UnlinkedHost {
    let _ = contract_identity();
    UnlinkedHost
}
