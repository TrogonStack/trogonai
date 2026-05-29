//! Wasmtime policy bundle surface (Block F). Runtime wiring lands in item 2.
//!
//! Stable Rust mirror of `wit/trogon-mcp-policy.wit` for linker and pool integration.

mod bindings;

pub use bindings::{
    contract_identity, ContractIdentity, HostFailure, HostImports, LogLevel, PolicyDecision,
    PolicyGuest, RequestCtx, SpanContext, SpicedbBinding, ToolDescriptor, UnlinkedHost,
    WIT_PACKAGE, WIT_VERSION, WORLD_NAME,
};

/// Returns an unlinked host stub and validates contract pins at call sites (item 2 replaces this).
#[must_use]
pub fn stub_host() -> UnlinkedHost {
    let _ = contract_identity();
    UnlinkedHost
}
