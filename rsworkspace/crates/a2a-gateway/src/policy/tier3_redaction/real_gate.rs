use std::sync::Arc;

use a2a_redaction::{
    output_is_tier3_refusal, tier3_refusal_reason_tag, RedactionError, SkillId, WasmRedactorHost,
};
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

struct SubstrateInvoker(Arc<crate::policy::WasmtimeSubstrate>);

impl Tier3PartInvoker for SubstrateInvoker {
    fn redact_part_bytes(&self, skill: &SkillId, payload: &[u8]) -> Result<Vec<u8>, RedactionError> {
        self.0.redaction.redact_part_bytes(skill, payload)
    }
}

impl RealTier3RedactionGate {
    pub fn new(invoker: Arc<dyn Tier3PartInvoker>) -> Self {
        Self { invoker }
    }

    pub fn from_host(host: Arc<WasmRedactorHost>) -> Self {
        Self::new(host)
    }

    pub fn from_substrate(substrate: Arc<crate::policy::WasmtimeSubstrate>) -> Self {
        Self::new(Arc::new(SubstrateInvoker(substrate)))
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
                return Tier3RedactionDecision::Refuse {
                    reason,
                    rule: skill_id,
                };
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
        RedactionError::Json(_) => Tier3EngineError::InvalidPayload,
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
    errors: std::collections::HashMap<SkillId, RedactionError>,
}

#[cfg(test)]
impl MockTier3PartInvoker {
    pub fn with_output(skill: SkillId, output: Vec<u8>) -> Arc<Self> {
        let mut outputs = std::collections::HashMap::new();
        outputs.insert(skill, output);
        Arc::new(Self {
            outputs,
            errors: std::collections::HashMap::new(),
        })
    }

    pub fn with_error(skill: SkillId, error: RedactionError) -> Arc<Self> {
        let mut errors = std::collections::HashMap::new();
        errors.insert(skill, error);
        Arc::new(Self {
            outputs: std::collections::HashMap::new(),
            errors,
        })
    }
}

#[cfg(test)]
impl Tier3PartInvoker for MockTier3PartInvoker {
    fn redact_part_bytes(&self, skill: &SkillId, payload: &[u8]) -> Result<Vec<u8>, RedactionError> {
        if let Some(err) = self.errors.get(skill) {
            return Err(match err {
                RedactionError::WasmCall(msg) => RedactionError::WasmCall(msg.clone()),
                RedactionError::WasmEngine(msg) => RedactionError::WasmEngine(msg.clone()),
                RedactionError::WasmModule(msg) => RedactionError::WasmModule(msg.clone()),
                RedactionError::WasmInstance(msg) => RedactionError::WasmInstance(msg.clone()),
                RedactionError::WasmAbi(msg) => RedactionError::WasmAbi(msg.clone()),
                RedactionError::WasmMemory(msg) => RedactionError::WasmMemory(msg.clone()),
                RedactionError::Json(msg) => RedactionError::Json(msg.clone()),
            });
        }
        Ok(self
            .outputs
            .get(skill)
            .cloned()
            .unwrap_or_else(|| payload.to_vec()))
    }
}
