//! Boot-resolved policy stack the dispatch orchestrator consumes.
//!
//! Three independently-loadable pieces — the Tier-2/Tier-3 wasmtime
//! substrate, the Tier-3 redaction gate, and the skill manifests
//! that drive Tier-3 — live behind one typed surface so the
//! dispatch path holds a single `&GatewayPolicyStack` instead of
//! three loose `Arc<_>` values it has to keep in sync.
//!
//! Stays focused on shape only; the env-driven boot helper that
//! decides which substrate/gate/manifests to instantiate is a
//! separate concern that the runtime entry point owns.

use std::collections::BTreeMap;
use std::sync::Arc;

use a2a_redaction::SkillId;

use crate::policy::wasmtime_substrate::WasmtimeSubstrate;
use crate::policy::{NoopTier3RedactionGate, Tier3RedactionGate, Tier3SkillManifest};

/// The dispatch path's view of the policy stack. Each field is
/// independently-replaceable in tests:
///
/// - `substrate` is `None` when no bundle is configured; the
///   orchestrator skips the Tier-2 CEL step instead of running
///   against a Noop evaluator hidden inside a phantom substrate.
/// - `tier3_gate` is always present (Noop when redaction is
///   disabled) so the dispatch flow never has to branch on
///   "do we have a gate at all" -- only on the decision the gate
///   returns.
/// - `tier3_manifests` is keyed by `SkillId` so the orchestrator
///   can stamp per-skill rewrites into the audit envelope without
///   reaching back into the gate.
pub struct GatewayPolicyStack {
    pub substrate: Option<Arc<WasmtimeSubstrate>>,
    pub tier3_gate: Arc<dyn Tier3RedactionGate>,
    pub tier3_manifests: BTreeMap<SkillId, Tier3SkillManifest>,
}

impl GatewayPolicyStack {
    /// Stack that lets everything through. Used by tests and by
    /// deployments that haven't configured a policy bundle so the
    /// dispatch path has a stable shape to take regardless of
    /// environment.
    #[must_use]
    pub fn noop() -> Self {
        Self {
            substrate: None,
            tier3_gate: Arc::new(NoopTier3RedactionGate),
            tier3_manifests: BTreeMap::new(),
        }
    }
}

#[cfg(test)]
mod tests;
