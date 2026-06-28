use std::sync::Arc;

use a2a_redaction::{RedactionError, SkillId, WasmRedactorHost, output_is_tier3_refusal, tier3_refusal_reason_tag};
use tracing::warn;

use super::context::Tier3EvaluationContext;
use super::decision::{Tier3EngineError, Tier3RedactionDecision, Tier3RefusalReason};
use super::gate::Tier3RedactionGate;
use super::json_path::{to_json_pointer, value_at_path};
use super::manifest::Tier3SkillManifest;
use super::rewrite::{RedactionRewrite, RewriteKind};

pub trait Tier3PartInvoker: Send + Sync {
    fn redact_part_bytes(&self, skill: &SkillId, payload: &[u8]) -> Result<Vec<u8>, RedactionError>;
}

impl Tier3PartInvoker for WasmRedactorHost {
    fn redact_part_bytes(&self, skill: &SkillId, payload: &[u8]) -> Result<Vec<u8>, RedactionError> {
        WasmRedactorHost::redact_part_bytes(self, skill, payload)
    }
}

pub struct RealTier3RedactionGate {
    invoker: Arc<dyn Tier3PartInvoker>,
}

impl RealTier3RedactionGate {
    pub fn new(invoker: Arc<dyn Tier3PartInvoker>) -> Self {
        Self { invoker }
    }

    /// Construct a Tier-3 gate from a standalone Wasm redactor host.
    /// The wasmtime-substrate-backed constructor lives in the slice
    /// that extracts `WasmtimeSubstrate` so this module doesn't have
    /// to drag the substrate type along just for one delegation.
    pub fn from_host(host: Arc<WasmRedactorHost>) -> Self {
        Self::new(host)
    }
}

impl Tier3RedactionGate for RealTier3RedactionGate {
    fn redact(&self, ctx: &mut Tier3EvaluationContext) -> Tier3RedactionDecision {
        let mut rewrites = Vec::new();
        let manifests: Vec<Tier3SkillManifest> = ctx.skill_manifests().values().cloned().collect();

        for manifest in manifests {
            let skill_id = manifest.skill_id().clone();
            let path = manifest.json_path().to_owned();

            let Some(before_value) = value_at_path(ctx.payload(), &path).cloned() else {
                warn!(
                    skill_id = %skill_id,
                    path = %path,
                    method = %ctx.method(),
                    "tier-3 manifest json_path missing from payload; skipping skill"
                );
                continue;
            };

            let input_bytes = match serde_json::to_vec(&before_value) {
                Ok(bytes) => bytes,
                Err(_) => {
                    return Tier3RedactionDecision::Error {
                        rule: skill_id,
                        kind: Tier3EngineError::InvalidPayload,
                    };
                }
            };

            let output_bytes = match self.invoker.redact_part_bytes(&skill_id, &input_bytes) {
                Ok(bytes) => bytes,
                // `WasmRedactorHost` on main detects the refusal sentinel
                // internally and surfaces it as `RedactionError::Tier3Refusal`
                // rather than passing the sentinel bytes through. Route
                // that into a `Refuse` decision (not `Error`) so the
                // audit subject records a policy denial, not an engine
                // crash.
                Err(RedactionError::Tier3Refusal(tag)) => {
                    let reason = tag
                        .as_deref()
                        .and_then(Tier3RefusalReason::from_sentinel_tag)
                        .unwrap_or(Tier3RefusalReason::SkillPolicyDeniedPart);
                    return Tier3RedactionDecision::Refuse { reason, rule: skill_id };
                }
                Err(err) => {
                    return Tier3RedactionDecision::Error {
                        rule: skill_id,
                        kind: map_redaction_error(&err),
                    };
                }
            };

            // Defensive sentinel check — older hosts emitted the refusal
            // sentinel directly in the output bytes instead of routing
            // it through `Tier3Refusal`. We still recognize that shape
            // so a guest that targets either ABI fails-closed the same
            // way.
            if output_is_tier3_refusal(&output_bytes) {
                if !output_bytes.starts_with(b"{") {
                    warn!(
                        skill_id = %skill_id,
                        path = %path,
                        "tier-3 skill returned refusal sentinel; guest output may have collided with sentinel prefix"
                    );
                }
                let reason = tier3_refusal_reason_tag(&output_bytes)
                    .and_then(Tier3RefusalReason::from_sentinel_tag)
                    .unwrap_or(Tier3RefusalReason::SkillPolicyDeniedPart);
                return Tier3RedactionDecision::Refuse { reason, rule: skill_id };
            }

            let after_value: serde_json::Value = match serde_json::from_slice(&output_bytes) {
                Ok(value) => value,
                Err(_) => {
                    return Tier3RedactionDecision::Error {
                        rule: skill_id,
                        kind: Tier3EngineError::InvalidPayload,
                    };
                }
            };

            if before_value == after_value {
                continue;
            }

            let kind = match manifest.kind() {
                RewriteKind::Replaced => RewriteKind::infer_from_values(&before_value, &after_value),
                other => other.clone(),
            };

            let Some(pointer) = to_json_pointer(&path) else {
                return Tier3RedactionDecision::Error {
                    rule: skill_id,
                    kind: Tier3EngineError::InvalidPayload,
                };
            };
            let Some(slot) = ctx.payload_mut().pointer_mut(&pointer) else {
                return Tier3RedactionDecision::Error {
                    rule: skill_id,
                    kind: Tier3EngineError::InvalidPayload,
                };
            };
            *slot = after_value;

            // RedactionRewrite::new validates path; reaching this point
            // already required value_at_path to resolve so the path is
            // non-empty by construction. The error branch here would
            // be a genuine engine bug.
            match RedactionRewrite::new(skill_id.clone(), path, kind) {
                Ok(rewrite) => rewrites.push(rewrite),
                Err(_) => {
                    return Tier3RedactionDecision::Error {
                        rule: skill_id,
                        kind: Tier3EngineError::InvalidPayload,
                    };
                }
            }
        }

        Tier3RedactionDecision::Allow { rewrites }
    }
}

fn map_redaction_error(err: &RedactionError) -> Tier3EngineError {
    match err {
        RedactionError::WasmCall(_) => Tier3EngineError::WasmTrap,
        // `Tier3Refusal` is handled separately at the call site (it
        // maps to a `Refuse` decision, not an engine error); reaching
        // this match arm with a refusal would be a routing bug, so we
        // still classify it as `InvalidPayload` to surface in the audit
        // chain rather than silently returning a phantom engine error.
        RedactionError::Json(_) | RedactionError::Tier3Refusal(_) | RedactionError::Signature(_) => {
            Tier3EngineError::InvalidPayload
        }
        RedactionError::WasmEngine(_)
        | RedactionError::WasmModule(_)
        | RedactionError::WasmInstance(_)
        | RedactionError::WasmAbi(_)
        | RedactionError::WasmMemory(_) => Tier3EngineError::WasmAbi,
    }
}

#[cfg(test)]
pub(crate) struct MockTier3PartInvoker {
    outputs: std::collections::HashMap<SkillId, Vec<u8>>,
    // Error factories rather than stored errors: `RedactionError` carries
    // a `serde_json::Error` in its `Json` variant which is not `Clone`,
    // and the `Signature` variant wraps a type we don't want to depend
    // on from tests. A factory closure lets the mock construct a fresh
    // error per call without needing `Clone`.
    error_factories: std::collections::HashMap<SkillId, Arc<dyn Fn() -> RedactionError + Send + Sync>>,
}

#[cfg(test)]
impl MockTier3PartInvoker {
    pub fn with_output(skill: SkillId, output: Vec<u8>) -> Arc<Self> {
        let mut outputs = std::collections::HashMap::new();
        outputs.insert(skill, output);
        Arc::new(Self {
            outputs,
            error_factories: std::collections::HashMap::new(),
        })
    }

    /// Register an error response for a skill, identified by which
    /// variant should fire. Variants that carry non-`Clone` sources
    /// (`Json`, `Signature`) panic — those aren't representable through
    /// the variant-tag dispatch the mock uses. Use `with_error_fn` if
    /// you need to inject one of those for a test.
    pub fn with_error(skill: SkillId, error: RedactionError) -> Arc<Self> {
        let factory = factory_for_variant(&error);
        let mut factories: std::collections::HashMap<SkillId, Arc<dyn Fn() -> RedactionError + Send + Sync>> =
            std::collections::HashMap::new();
        factories.insert(skill, factory);
        Arc::new(Self {
            outputs: std::collections::HashMap::new(),
            error_factories: factories,
        })
    }
}

#[cfg(test)]
fn factory_for_variant(err: &RedactionError) -> Arc<dyn Fn() -> RedactionError + Send + Sync> {
    match err {
        RedactionError::WasmCall(_) => Arc::new(|| RedactionError::WasmCall("mock trap".into())),
        RedactionError::WasmEngine(_) => Arc::new(|| RedactionError::WasmEngine("mock engine".into())),
        RedactionError::WasmModule(_) => Arc::new(|| RedactionError::WasmModule("mock module".into())),
        RedactionError::WasmInstance(_) => Arc::new(|| RedactionError::WasmInstance("mock instance".into())),
        RedactionError::WasmAbi(_) => Arc::new(|| RedactionError::WasmAbi("mock abi".into())),
        RedactionError::WasmMemory(_) => Arc::new(|| RedactionError::WasmMemory("mock memory".into())),
        RedactionError::Tier3Refusal(tag) => {
            let tag = tag.clone();
            Arc::new(move || RedactionError::Tier3Refusal(tag.clone()))
        }
        // `Json` and `Signature` wrap non-`Clone` source types we can't
        // rebuild deterministically without smuggling extra deps into
        // the gateway crate. Panicking here surfaces a test-author bug
        // loudly instead of silently dispatching to a WasmAbi fallback,
        // which used to mask wrong-variant assertions.
        RedactionError::Json(_) => {
            panic!(
                "MockTier3PartInvoker::with_error does not support RedactionError::Json — wrap the test invoker manually"
            )
        }
        RedactionError::Signature(_) => {
            panic!(
                "MockTier3PartInvoker::with_error does not support RedactionError::Signature — wrap the test invoker manually"
            )
        }
    }
}

#[cfg(test)]
impl Tier3PartInvoker for MockTier3PartInvoker {
    fn redact_part_bytes(&self, skill: &SkillId, payload: &[u8]) -> Result<Vec<u8>, RedactionError> {
        if let Some(factory) = self.error_factories.get(skill) {
            return Err(factory());
        }
        Ok(self.outputs.get(skill).cloned().unwrap_or_else(|| payload.to_vec()))
    }
}
