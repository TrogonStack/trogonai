use sha2::{Digest, Sha256};

use crate::agent_id::A2aAgentId;

/// Gateway ingress subject rewrite recorded on forward decision sites.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuditSubjectRewrite(String);

impl AuditSubjectRewrite {
    pub fn new(ingress_subject: &str, agent_subject: &str) -> Self {
        Self(format!("ingress:{ingress_subject} -> agent:{agent_subject}"))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn into_audit_json(self) -> serde_json::Value {
        serde_json::Value::Array(vec![serde_json::Value::String(self.0)])
    }
}

/// Stable JetStream consumer name derived for SSE-shaped gateway forwards.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GatewayStreamConsumerName(String);

impl GatewayStreamConsumerName {
    pub fn for_sse_method(agent_id: &A2aAgentId, method_dots: &str) -> Option<Self> {
        match method_dots {
            "message.stream" | "tasks.resubscribe" => {
                Some(Self(format!("gateway.{}.{}", agent_id.as_str(), method_dots)))
            }
            _ => None,
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Optional audit extras populated when the gateway forwards ingress to an agent RPC subject.
pub fn gateway_forward_audit_extras(
    ingress_subject: &str,
    agent_subject: &str,
    agent_id: &A2aAgentId,
    method_dots: &str,
) -> (Option<serde_json::Value>, Option<String>) {
    let rewrites = Some(AuditSubjectRewrite::new(ingress_subject, agent_subject).into_audit_json());
    let stream_consumer =
        GatewayStreamConsumerName::for_sse_method(agent_id, method_dots).map(|name| name.as_str().to_owned());
    (rewrites, stream_consumer)
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum AuditOutcome {
    Ok,
    Err { code: i32, message: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Tier1Decision {
    Allow,
    Deny,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Tier3Decision {
    Allow,
    Refuse,
    Error,
}

/// Optional forward-compat fields for [`AuditEnvelope`].
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct AuditEnvelopeFields {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trace_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rules_fired: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rewrites: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream_consumer: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub zed_token_snapshot: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub caller_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub caller_source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tier1_decision: Option<Tier1Decision>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tier3_decision: Option<Tier3Decision>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct AuditEnvelope {
    pub agent_id: String,
    pub method: String,
    pub req_id: Option<String>,
    pub started_at: u64,
    pub latency_ms: u64,
    #[serde(flatten)]
    pub outcome: AuditOutcome,
    pub params_fingerprint: Option<String>,
    #[serde(flatten)]
    pub extras: AuditEnvelopeFields,
}

impl AuditEnvelope {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        agent_id: &A2aAgentId,
        method: impl Into<String>,
        req_id: Option<String>,
        started_at: u64,
        latency_ms: u64,
        outcome: AuditOutcome,
        raw_params: Option<&[u8]>,
        extras: AuditEnvelopeFields,
    ) -> Self {
        let params_fingerprint = raw_params.filter(|b| !b.is_empty()).map(|b| {
            let hash = Sha256::digest(b);
            hash.iter().fold(String::with_capacity(64), |mut acc, byte| {
                use std::fmt::Write as _;
                let _ = write!(acc, "{byte:02x}");
                acc
            })
        });
        Self {
            agent_id: agent_id.as_str().to_string(),
            method: method.into(),
            req_id,
            started_at,
            latency_ms,
            outcome,
            params_fingerprint,
            extras,
        }
    }
}

#[cfg(test)]
mod tests;
