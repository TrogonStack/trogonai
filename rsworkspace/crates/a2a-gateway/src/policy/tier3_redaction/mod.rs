mod context;
mod decision;
mod gate;
mod json_path;
mod manifest;
mod real_gate;
mod rewrite;

#[cfg(test)]
mod tests;

pub use context::Tier3EvaluationContext;
pub use decision::{Tier3EngineError, Tier3RedactionDecision, Tier3RefusalReason};
pub use gate::{NoopTier3RedactionGate, Tier3RedactionGate};
pub use manifest::{load_tier3_manifests_from_bundle, Tier3SkillManifest};
pub use real_gate::RealTier3RedactionGate;
pub use rewrite::{RedactionRewrite, RewriteKind};

pub fn gateway_tier3_redaction_enabled<E: trogon_std::env::ReadEnv>(env: &E) -> bool {
    let Ok(flag) = env.var("A2A_GATEWAY_TIER3_REDACTION_ENABLED") else {
        return false;
    };
    matches!(
        flag.to_ascii_lowercase().as_str(),
        "1" | "true" | "yes" | "on"
    )
}

pub fn tier3_redaction_audit_rewrites(rewrites: &[RedactionRewrite]) -> Option<serde_json::Value> {
    if rewrites.is_empty() {
        return None;
    }
    Some(serde_json::Value::Array(
        rewrites
            .iter()
            .map(|rewrite| serde_json::Value::String(rewrite.to_string()))
            .collect(),
    ))
}

pub fn merge_forward_audit_rewrites(
    tier3_rewrites: &[RedactionRewrite],
    ingress_subject: &str,
    agent_subject: &str,
    agent_id: &a2a_nats::A2aAgentId,
    method_dots: &str,
) -> (Option<serde_json::Value>, Option<String>) {
    let (route_rewrites, stream_consumer) =
        a2a_nats::audit::envelope::gateway_forward_audit_extras(ingress_subject, agent_subject, agent_id, method_dots);

    let Some(route_array) = route_rewrites else {
        return (tier3_redaction_audit_rewrites(tier3_rewrites), stream_consumer);
    };

    let Some(tier3_array) = tier3_redaction_audit_rewrites(tier3_rewrites) else {
        return (Some(route_array), stream_consumer);
    };

    let mut merged = route_array.as_array().cloned().unwrap_or_default();
    if let Some(extra) = tier3_array.as_array() {
        merged.extend(extra.iter().cloned());
    }
    (Some(serde_json::Value::Array(merged)), stream_consumer)
}
