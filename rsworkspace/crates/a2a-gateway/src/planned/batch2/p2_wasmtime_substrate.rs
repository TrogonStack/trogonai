//! Phase 2 — Single **Wasmtime** engine in gateway (Tier 2 CEL→WASM bundle eval + Tier 3 redaction WASM).
//!
//! Compile-only seam for the shared embedded Wasmtime host **without** pulling the `wasmtime` crate into
//! `a2a-gateway` until bundle loading and signing land.
//!
//! ## Cross-reference (do not import)
//!
//! The parent [`crate::planned::policy_wasmtime_substrate`] module sketches the same engine/pool model
//! with unprefixed types. This batch-2 module duplicates the minimal trait surface with **`P2*`** names
//! so both compile together without name collisions:
//!
//! | Policy substrate | Phase 2 batch seam |
//! |------------------|--------------------|
//! | [`Tier2CelWasm`](crate::planned::policy_wasmtime_substrate::Tier2CelWasm) | [`P2CelRuleKey`] |
//! | [`Tier3RedactionWasm`](crate::planned::policy_wasmtime_substrate::Tier3RedactionWasm) | [`P2RedactionSkill`] |
//! | [`WasmHostPool`](crate::planned::policy_wasmtime_substrate::WasmHostPool) | [`P2WasmHost`] |
//! | [`PolicyModuleHandle`](crate::planned::policy_wasmtime_substrate::PolicyModuleHandle) | per-call module traits below |
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
//! [`P2WasmHost`] resolves Tier 2 and Tier 3 module handles from the **same** engine; only the module
//! artifact and host imports differ. CEL programs compile to WASM at **bundle build** time; the gateway
//! never hosts a CEL interpreter.
//!
//! ## Invoke stages
//!
//! Tier 2 runs at both unary and streaming ingress sites; Tier 3 redaction applies when materializing
//! outbound `Message.parts` / `Artifact.parts`. [`P2InvokeStage`] tags which ingress path acquired the
//! store so audit and deadline planners ([`super::p2_message_send_deadline`],
//! [`super::p2_streaming_backpressure`]) can correlate WASM fuel/time budgets.

use std::fmt;

/// Ingress site that triggered a Tier 2 / Tier 3 WASM invocation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum P2InvokeStage {
    /// Unary `message/send` (or other gateway unary RPC) before opaque forward.
    IngressUnary,
    /// Per-chunk evaluation on gateway pull-consumer egress for `message/stream` / task events.
    IngressStreamChunk,
}

impl fmt::Display for P2InvokeStage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::IngressUnary => f.write_str("ingress_unary"),
            Self::IngressStreamChunk => f.write_str("ingress_stream_chunk"),
        }
    }
}

/// Tier 2 module selector — CEL policy compiled to WASM when the bundle is built.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum P2CelRuleKey {
    /// Bundle-relative rule name (for example `ingress.message_send`).
    RuleId(String),
    /// No Tier 2 module configured for this evaluation site.
    Unconfigured,
}

/// Tier 3 module selector — skill-id-keyed redaction WASM from the bundle.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum P2RedactionSkill {
    /// AgentCard skill id whose redaction program should run over `parts[*]`.
    SkillId(String),
    /// Pass parts through without invoking redaction WASM.
    Passthrough,
}

/// Outcome of a Tier 2 CEL→WASM evaluation at an ingress site.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum P2CelVerdict {
    Allow,
    Deny { reason: String },
}

/// Process-wide Wasmtime host: one engine, many bundle modules, isolated Stores per call.
pub trait P2WasmHost: Send + Sync {
    type CelModule: P2CelWasmModule;
    type RedactionModule: P2RedactionWasmModule;
    type Error: std::error::Error + Send + Sync + 'static;

    /// Resolve a Tier 2 CEL→WASM module handle for the given bundle rule key.
    fn resolve_cel(&self, rule: P2CelRuleKey) -> Result<Self::CelModule, Self::Error>;

    /// Resolve a Tier 3 skill-keyed redaction WASM module handle.
    fn resolve_redaction(&self, skill: P2RedactionSkill) -> Result<Self::RedactionModule, Self::Error>;
}

/// Tier 2 CEL→WASM evaluation over JSON policy context at an ingress site.
pub trait P2CelWasmModule: Send {
    type Error: std::error::Error + Send + Sync + 'static;

    fn rule_key(&self) -> &P2CelRuleKey;

    /// Evaluate compiled CEL WASM against a JSON context blob for the given ingress stage.
    fn evaluate(
        &self,
        stage: P2InvokeStage,
        context_json: &[u8],
    ) -> Result<P2CelVerdict, Self::Error>;
}

/// Tier 3 WASM redaction over `Message.parts` / `Artifact.parts` JSON at an ingress site.
pub trait P2RedactionWasmModule: Send {
    type Error: std::error::Error + Send + Sync + 'static;

    fn skill_key(&self) -> &P2RedactionSkill;

    /// Redact or rewrite parts JSON for the given ingress stage; returns updated parts JSON.
    fn redact(&self, stage: P2InvokeStage, parts_json: &[u8]) -> Result<Vec<u8>, Self::Error>;
}

/// Placeholder error until real Wasmtime wiring lands.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct P2StubWasmError;

impl fmt::Display for P2StubWasmError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("phase 2 wasmtime substrate not wired")
    }
}

impl std::error::Error for P2StubWasmError {}

/// Compile-only Tier 2 module stub exercising [`P2CelWasmModule`].
#[derive(Debug, Clone)]
pub struct P2StubCelWasmModule {
    rule: P2CelRuleKey,
}

impl P2CelWasmModule for P2StubCelWasmModule {
    type Error = P2StubWasmError;

    fn rule_key(&self) -> &P2CelRuleKey {
        &self.rule
    }

    fn evaluate(
        &self,
        _stage: P2InvokeStage,
        _context_json: &[u8],
    ) -> Result<P2CelVerdict, Self::Error> {
        Ok(P2CelVerdict::Allow)
    }
}

/// Compile-only Tier 3 module stub exercising [`P2RedactionWasmModule`].
#[derive(Debug, Clone)]
pub struct P2StubRedactionWasmModule {
    skill: P2RedactionSkill,
}

impl P2RedactionWasmModule for P2StubRedactionWasmModule {
    type Error = P2StubWasmError;

    fn skill_key(&self) -> &P2RedactionSkill {
        &self.skill
    }

    fn redact(
        &self,
        _stage: P2InvokeStage,
        parts_json: &[u8],
    ) -> Result<Vec<u8>, Self::Error> {
        Ok(parts_json.to_vec())
    }
}

/// Compile-only host stub exercising [`P2WasmHost`].
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct P2StubWasmHost;

impl P2WasmHost for P2StubWasmHost {
    type CelModule = P2StubCelWasmModule;
    type RedactionModule = P2StubRedactionWasmModule;
    type Error = P2StubWasmError;

    fn resolve_cel(&self, rule: P2CelRuleKey) -> Result<Self::CelModule, Self::Error> {
        Ok(P2StubCelWasmModule { rule })
    }

    fn resolve_redaction(&self, skill: P2RedactionSkill) -> Result<Self::RedactionModule, Self::Error> {
        Ok(P2StubRedactionWasmModule { skill })
    }
}
