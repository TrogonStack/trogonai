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
                Err(err) => {
                    return Tier3RedactionDecision::Error {
                        rule: skill_id,
                        kind: map_redaction_error(&err),
                    };
                }
            };

            if output_is_tier3_refusal(&output_bytes) {
                if !output_bytes.starts_with(b"{") {
                    warn!(
                        skill_id = %skill_id,
                        path = %path,
                        "tier-3 skill returned refusal sentinel; guest output may have collided with sentinel prefix"
                    );
                }
                let reason = tier3_refusal_reason_tag(&output_bytes)
                    .map(Tier3RefusalReason::from_sentinel_tag)
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

            rewrites.push(RedactionRewrite::new(skill_id, path, kind));
        }

        Tier3RedactionDecision::Allow { rewrites }
    }
}

fn map_redaction_error(err: &RedactionError) -> Tier3EngineError {
    match err {
        RedactionError::WasmCall(_) => Tier3EngineError::WasmTrap,
        // A Tier3Refusal here is unexpected — the sentinel-detection
        // branch above handles refusals before the invoker errors. Map
        // it to InvalidPayload as a defensive fallback so the audit
        // surface still tags the rule that refused.
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

    pub fn with_error(skill: SkillId, error: RedactionError) -> Arc<Self> {
        // The error factory rebuilds an equivalent error each call so
        // the mock doesn't depend on `Clone` for `RedactionError`.
        let label = error_label(&error);
        let mut factories: std::collections::HashMap<SkillId, Arc<dyn Fn() -> RedactionError + Send + Sync>> =
            std::collections::HashMap::new();
        factories.insert(
            skill,
            Arc::new(move || rebuild_error(label)) as Arc<dyn Fn() -> RedactionError + Send + Sync>,
        );
        Arc::new(Self {
            outputs: std::collections::HashMap::new(),
            error_factories: factories,
        })
    }
}

/// Tag for which `RedactionError` variant the mock should produce. We
/// keep this as a string label rather than the variant itself because
/// `RedactionError`'s variants carry sources (serde_json::Error,
/// SignatureVerificationError) that aren't trivially cloneable.
#[cfg(test)]
fn error_label(err: &RedactionError) -> &'static str {
    match err {
        RedactionError::WasmCall(_) => "wasm_call",
        RedactionError::WasmEngine(_) => "wasm_engine",
        RedactionError::WasmModule(_) => "wasm_module",
        RedactionError::WasmInstance(_) => "wasm_instance",
        RedactionError::WasmAbi(_) => "wasm_abi",
        RedactionError::WasmMemory(_) => "wasm_memory",
        RedactionError::Json(_) => "json",
        RedactionError::Tier3Refusal(_) => "tier3_refusal",
        RedactionError::Signature(_) => "signature",
    }
}

#[cfg(test)]
fn rebuild_error(label: &'static str) -> RedactionError {
    match label {
        "wasm_call" => RedactionError::WasmCall("mock trap".into()),
        "wasm_engine" => RedactionError::WasmEngine("mock engine".into()),
        "wasm_module" => RedactionError::WasmModule("mock module".into()),
        "wasm_instance" => RedactionError::WasmInstance("mock instance".into()),
        "wasm_abi" => RedactionError::WasmAbi("mock abi".into()),
        "wasm_memory" => RedactionError::WasmMemory("mock memory".into()),
        _ => RedactionError::WasmAbi("mock fallback".into()),
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
