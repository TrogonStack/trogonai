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
    let stream_consumer = GatewayStreamConsumerName::for_sse_method(agent_id, method_dots)
        .map(|name| name.as_str().to_owned());
    (rewrites, stream_consumer)
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum AuditOutcome {
    Ok,
    Err { code: i32, message: String },
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
    pub refusal_skill: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub caller_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub caller_source: Option<String>,
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
mod tests {
    use super::*;

    fn agent() -> A2aAgentId {
        A2aAgentId::new("test-agent").unwrap()
    }

    #[test]
    fn ok_outcome_serializes_correctly() {
        let env = AuditEnvelope::new(
            &agent(),
            "message/send",
            Some("r1".into()),
            1000,
            5,
            AuditOutcome::Ok,
            None,
            AuditEnvelopeFields::default(),
        );
        let v = serde_json::to_value(&env).unwrap();
        assert_eq!(v["outcome"], "ok");
        assert_eq!(v["agent_id"], "test-agent");
        assert_eq!(v["method"], "message/send");
        assert_eq!(v["req_id"], "r1");
        assert_eq!(v["latency_ms"], 5);
        assert!(v["params_fingerprint"].is_null());
        assert!(v.get("trace_id").is_none());
        assert!(v.get("rules_fired").is_none());
        assert!(v.get("rewrites").is_none());
        assert!(v.get("stream_consumer").is_none());
    }

    #[test]
    fn err_outcome_serializes_correctly() {
        let env = AuditEnvelope::new(
            &agent(),
            "tasks/get",
            None,
            2000,
            10,
            AuditOutcome::Err {
                code: -32001,
                message: "not found".into(),
            },
            Some(b"some params"),
            AuditEnvelopeFields::default(),
        );
        let v = serde_json::to_value(&env).unwrap();
        assert_eq!(v["outcome"], "err");
        assert_eq!(v["code"], -32001);
        assert_eq!(v["message"], "not found");
        assert!(v["params_fingerprint"].is_string());
        assert!(!v["params_fingerprint"].as_str().unwrap().is_empty());
    }

    #[test]
    fn params_fingerprint_is_none_for_empty_params() {
        let env = AuditEnvelope::new(
            &agent(),
            "tasks/get",
            None,
            0,
            0,
            AuditOutcome::Ok,
            Some(b""),
            AuditEnvelopeFields::default(),
        );
        assert!(env.params_fingerprint.is_none());
    }

    #[test]
    fn params_fingerprint_is_deterministic() {
        let a = AuditEnvelope::new(
            &agent(),
            "m",
            None,
            0,
            0,
            AuditOutcome::Ok,
            Some(b"hello"),
            AuditEnvelopeFields::default(),
        );
        let b = AuditEnvelope::new(
            &agent(),
            "m",
            None,
            0,
            0,
            AuditOutcome::Ok,
            Some(b"hello"),
            AuditEnvelopeFields::default(),
        );
        assert_eq!(a.params_fingerprint, b.params_fingerprint);
    }

    #[test]
    fn params_fingerprint_differs_for_different_params() {
        let a = AuditEnvelope::new(
            &agent(),
            "m",
            None,
            0,
            0,
            AuditOutcome::Ok,
            Some(b"hello"),
            AuditEnvelopeFields::default(),
        );
        let b = AuditEnvelope::new(
            &agent(),
            "m",
            None,
            0,
            0,
            AuditOutcome::Ok,
            Some(b"world"),
            AuditEnvelopeFields::default(),
        );
        assert_ne!(a.params_fingerprint, b.params_fingerprint);
    }

    #[test]
    fn audit_subject_rewrite_formats_ingress_to_agent() {
        let rewrite = AuditSubjectRewrite::new("a2a.gateway.bot.message.send", "a2a.agent.bot.message.send");
        assert_eq!(
            rewrite.as_str(),
            "ingress:a2a.gateway.bot.message.send -> agent:a2a.agent.bot.message.send"
        );
        let json = rewrite.into_audit_json();
        assert_eq!(
            json,
            serde_json::json!(["ingress:a2a.gateway.bot.message.send -> agent:a2a.agent.bot.message.send"])
        );
    }

    #[test]
    fn gateway_stream_consumer_name_for_sse_methods_only() {
        let agent = agent();
        assert_eq!(
            GatewayStreamConsumerName::for_sse_method(&agent, "message.stream")
                .unwrap()
                .as_str(),
            "gateway.test-agent.message.stream"
        );
        assert_eq!(
            GatewayStreamConsumerName::for_sse_method(&agent, "tasks.resubscribe")
                .unwrap()
                .as_str(),
            "gateway.test-agent.tasks.resubscribe"
        );
        assert!(GatewayStreamConsumerName::for_sse_method(&agent, "message.send").is_none());
        assert!(GatewayStreamConsumerName::for_sse_method(&agent, "tasks.get").is_none());
    }

    #[test]
    fn gateway_forward_audit_extras_populates_rewrite_and_sse_consumer() {
        let agent = agent();
        let (rewrites, stream_consumer) = gateway_forward_audit_extras(
            "a2a.gateway.bot.message.stream",
            "a2a.agent.bot.message.stream",
            &agent,
            "message.stream",
        );
        assert_eq!(
            rewrites,
            Some(serde_json::json!(["ingress:a2a.gateway.bot.message.stream -> agent:a2a.agent.bot.message.stream"]))
        );
        assert_eq!(stream_consumer.as_deref(), Some("gateway.test-agent.message.stream"));
    }

    #[test]
    fn gateway_forward_audit_extras_omits_stream_consumer_for_unary() {
        let agent = agent();
        let (rewrites, stream_consumer) = gateway_forward_audit_extras(
            "a2a.gateway.bot.message.send",
            "a2a.agent.bot.message.send",
            &agent,
            "message.send",
        );
        assert_eq!(
            rewrites,
            Some(serde_json::json!(["ingress:a2a.gateway.bot.message.send -> agent:a2a.agent.bot.message.send"]))
        );
        assert!(stream_consumer.is_none());
    }

    #[test]
    fn optional_fields_omit_when_none_and_include_trace_id_when_set() {
        let default_env = AuditEnvelope::new(
            &agent(),
            "message/send",
            None,
            0,
            0,
            AuditOutcome::Ok,
            None,
            AuditEnvelopeFields::default(),
        );
        let default_json = serde_json::to_value(&default_env).unwrap();
        assert!(default_json.get("trace_id").is_none());

        let extras = AuditEnvelopeFields {
            trace_id: Some("trace-abc".into()),
            ..Default::default()
        };
        let traced_env = AuditEnvelope::new(&agent(), "message/send", None, 0, 0, AuditOutcome::Ok, None, extras);
        let traced_json = serde_json::to_value(&traced_env).unwrap();
        assert_eq!(traced_json["trace_id"], "trace-abc");
        assert!(traced_json.get("rules_fired").is_none());
        assert!(traced_json.get("rewrites").is_none());
        assert!(traced_json.get("stream_consumer").is_none());
    }
}
