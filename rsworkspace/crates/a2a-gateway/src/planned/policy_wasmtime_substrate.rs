//! Policy substrate — single **Wasmtime** runtime in gateway (Tier 2 CEL WASM + Tier 3 redaction WASM).
//!
//! Compile-only seam: trait boundaries for the future embedded Wasmtime host **without** pulling the
//! `wasmtime` crate into `a2a-gateway` until bundle loading and signing land.
//!
//! ## Shared engine concurrency
//!
//! Wasmtime's `Engine` is `Send + Sync` and cheap to clone (internally reference-counted). The gateway
//! should construct **one** process-wide engine at startup and share clones across ingress workers.
//!
//! | Artifact | Sharing model |
//! |----------|----------------|
//! | `Engine` | One logical instance, cloned per worker thread if needed |
//! | Compiled `Module` | Loaded once per bundle revision; immutable; shared across all Stores |
//! | `Store` / `Instance` | **Not** shared across concurrent calls — one isolated store per in-flight evaluation |
//!
//! [`WasmHostPool`] bounds concurrent Stores (memory + fuel limits) and avoids cold-start latency by
//! reusing returned slots. Tier 2 (CEL→WASM) and Tier 3 (redaction WASM) draw from the **same** engine;
//! only the module handle and host imports differ.
//!
//! Callers must treat [`PolicyModuleHandle`] as a lease: acquire before evaluate/redact, release when
//! done. Holding handles across `.await` points consumes pool capacity and can stall ingress under
//! burst load — align pool depth with unary deadline and pull-consumer backpressure knobs (see
//! [`super::gw_unary_deadline`], [`super::gw_pull_backpressure`]).
//!
//! CEL programs compile to WASM at **bundle build** time; the gateway never hosts a CEL interpreter.

use std::fmt;

/// Tier 2 module selector — CEL policy compiled to WASM when the bundle is built.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Tier2CelWasm {
    /// Bundle-relative rule name (for example `ingress.message_send`).
    RuleId(String),
    /// No Tier 2 module configured for this evaluation site.
    Unconfigured,
}

/// Tier 3 module selector — skill-id-keyed redaction WASM from the bundle.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Tier3RedactionWasm {
    /// AgentCard skill id whose redaction program should run over `parts[*]`.
    SkillId(String),
    /// Pass parts through without invoking redaction WASM.
    Passthrough,
}

/// Borrowed slot for a single in-flight WASM policy invocation.
pub trait PolicyModuleHandle: Send {
    /// Which tier/module this handle was acquired for.
    fn tier(&self) -> PolicyModuleKind;
}

/// Active policy tier for a borrowed handle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PolicyModuleKind {
    Cel(Tier2CelWasm),
    Redaction(Tier3RedactionWasm),
}

/// Pool of reusable Wasmtime Stores backed by one shared engine instance.
pub trait WasmHostPool: Send + Sync {
    type Handle: PolicyModuleHandle;
    type Error: std::error::Error + Send + Sync + 'static;

    /// Reserve an isolated store for Tier 2 CEL→WASM evaluation.
    fn acquire_tier2(&self, module: Tier2CelWasm) -> Result<Self::Handle, Self::Error>;

    /// Reserve an isolated store for Tier 3 redaction WASM.
    fn acquire_tier3(&self, module: Tier3RedactionWasm) -> Result<Self::Handle, Self::Error>;
}

/// Placeholder error until real Wasmtime wiring lands.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct StubPolicyError;

impl fmt::Display for StubPolicyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("wasm policy substrate not wired")
    }
}

impl std::error::Error for StubPolicyError {}

/// No-op handle returned by [`StubWasmHostPool`].
pub struct StubPolicyHandle {
    tier: PolicyModuleKind,
}

impl PolicyModuleHandle for StubPolicyHandle {
    fn tier(&self) -> PolicyModuleKind {
        self.tier.clone()
    }
}

/// Compile-only pool stub exercising the trait surface.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct StubWasmHostPool;

impl WasmHostPool for StubWasmHostPool {
    type Handle = StubPolicyHandle;
    type Error = StubPolicyError;

    fn acquire_tier2(&self, module: Tier2CelWasm) -> Result<Self::Handle, Self::Error> {
        Ok(StubPolicyHandle {
            tier: PolicyModuleKind::Cel(module),
        })
    }

    fn acquire_tier3(&self, module: Tier3RedactionWasm) -> Result<Self::Handle, Self::Error> {
        Ok(StubPolicyHandle {
            tier: PolicyModuleKind::Redaction(module),
        })
    }
}
