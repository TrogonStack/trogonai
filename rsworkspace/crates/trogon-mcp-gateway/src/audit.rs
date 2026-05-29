//! JetStream-backed audit publishing for gateway decisions.

use std::time::Duration;

use async_nats::jetstream;
use async_nats::jetstream::stream;
use serde::{Deserialize, Serialize};
use tracing::warn;

use crate::authz::IdentitySource;
use crate::redaction::RewriteEntry;

/// One redaction rewrite attestation for audit consumers (path and op only; no plaintext).
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct AuditRedactionRewrite {
    pub path: String,
    pub op: String,
}

impl From<&RewriteEntry> for AuditRedactionRewrite {
    fn from(entry: &RewriteEntry) -> Self {
        Self {
            path: entry.path.clone(),
            op: entry.op.clone(),
        }
    }
}

pub const AUDIT_OUTCOME_REDACTED: &str = "redacted";
pub const AUDIT_OUTCOME_REDACTION_SKIPPED: &str = "redaction_skipped";

/// One hop in a delegation chain (`act_chain` JWT claim / audit embedding).
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct AuditActChainEntry {
    pub sub: String,
    pub agent_id: String,
    pub wkl: String,
    pub iat: i64,
}

/// Optional identity claims for audit embedding (mirrors JWT claim shape; independent of `jwt.rs`).
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct IdentityFields {
    pub agent_id: Option<String>,
    pub agent_version: Option<String>,
    pub wkl: Option<String>,
    pub purpose: Option<String>,
    pub session_id: Option<String>,
    pub act_chain: Option<Vec<AuditActChainEntry>>,
}

#[derive(Debug, Serialize)]
pub struct AuditEnvelope {
    pub subject_in: String,
    pub subject_out: String,
    pub outcome: &'static str,
    pub direction: &'static str,
    pub jsonrpc_method: String,
    pub tenant: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub caller_sub: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub jwt_issuer: Option<String>,
    pub identity_source: IdentitySource,
    pub request_id: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wkl: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub purpose: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub act_chain: Option<Vec<AuditActChainEntry>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rate_limit_scope: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry_after_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rewrites: Option<Vec<AuditRedactionRewrite>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub redaction_skip_reason: Option<String>,
}

impl AuditEnvelope {
    #[must_use]
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        subject_in: String,
        subject_out: String,
        outcome: &'static str,
        direction: &'static str,
        jsonrpc_method: String,
        tenant: Option<String>,
        caller_sub: Option<String>,
        jwt_issuer: Option<String>,
        identity_source: IdentitySource,
        request_id: Option<serde_json::Value>,
        identity: Option<&IdentityFields>,
    ) -> Self {
        let mut envelope = Self {
            subject_in,
            subject_out,
            outcome,
            direction,
            jsonrpc_method,
            tenant,
            caller_sub,
            jwt_issuer,
            identity_source,
            request_id,
            agent_id: None,
            agent_version: None,
            wkl: None,
            purpose: None,
            session_id: None,
            act_chain: None,
            rate_limit_scope: None,
            retry_after_ms: None,
            rewrites: None,
            redaction_skip_reason: None,
        };
        envelope.apply_identity_fields(identity);
        envelope
    }

    /// Populate optional identity fields from parsed claims (or `None` for shadow-mode / pre-identity rollout).
    pub fn apply_identity_fields(&mut self, identity: Option<&IdentityFields>) {
        let Some(id) = identity else {
            return;
        };
        self.agent_id = id.agent_id.clone();
        self.agent_version = id.agent_version.clone();
        self.wkl = id.wkl.clone();
        self.purpose = id.purpose.clone();
        self.session_id = id.session_id.clone();
        self.act_chain = id.act_chain.clone();
    }

    pub fn apply_rate_limit_fields(&mut self, scope: &str, retry_after_ms: u64) {
        self.rate_limit_scope = Some(scope.to_string());
        self.retry_after_ms = Some(retry_after_ms);
    }

    pub fn apply_rewrites(&mut self, rewrites: &[RewriteEntry]) {
        if rewrites.is_empty() {
            return;
        }
        self.rewrites = Some(rewrites.iter().map(AuditRedactionRewrite::from).collect());
    }

    pub fn apply_redaction_skip_reason(&mut self, reason: impl Into<String>) {
        self.redaction_skip_reason = Some(reason.into());
    }
}

pub fn audit_publish_subject(prefix: &str, outcome: &str, direction: &str, method_root: &str) -> String {
    format!("{prefix}.audit.{outcome}.{direction}.{method_root}")
}

pub fn jsonrpc_method_root(method: &str) -> String {
    method
        .split('/')
        .next()
        .filter(|s| !s.is_empty())
        .unwrap_or("unknown")
        .replace('.', "_")
}

pub async fn ensure_audit_stream(
    jetstream: &jetstream::Context,
    stream_name: &str,
    prefix: &str,
) -> Result<(), jetstream::context::CreateStreamError> {
    let subject = format!("{prefix}.audit.>");
    jetstream
        .get_or_create_stream(stream::Config {
            name: stream_name.to_string(),
            subjects: vec![subject],
            max_messages: 100_000,
            ..Default::default()
        })
        .await?;
    Ok(())
}

pub async fn publish_audit(
    jetstream: &jetstream::Context,
    subject: String,
    envelope: &AuditEnvelope,
    ack_timeout: Duration,
) {
    let body = match serde_json::to_vec(envelope) {
        Ok(b) => b,
        Err(e) => {
            warn!(error = %e, "audit envelope serialization failed");
            return;
        }
    };
    let Ok(pending) = jetstream.publish(subject, body.into()).await else {
        warn!("audit jetstream publish failed");
        return;
    };
    match tokio::time::timeout(ack_timeout, pending).await {
        Ok(Ok(_)) => {}
        Ok(Err(e)) => warn!(error = %e, "audit publish ack failed"),
        Err(_) => warn!(?ack_timeout, "audit publish ack timed out"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::authz::IdentitySource;

    fn sample_core_envelope() -> AuditEnvelope {
        AuditEnvelope::new(
            "mcp.gateway.request.fs.tools.call".to_string(),
            "mcp.server.fs.tools.call".to_string(),
            "allow",
            "request",
            "tools/call".to_string(),
            Some("acme".to_string()),
            Some("user:alice".to_string()),
            Some("https://issuer.example".to_string()),
            IdentitySource::Jwt,
            Some(serde_json::json!("req-1")),
            None,
        )
    }

    #[test]
    fn default_envelope_json_matches_prior_format() {
        let envelope = sample_core_envelope();
        let json = serde_json::to_value(&envelope).expect("serialize");
        let obj = json.as_object().expect("object");
        assert!(!obj.contains_key("agent_id"));
        assert!(!obj.contains_key("agent_version"));
        assert!(!obj.contains_key("wkl"));
        assert!(!obj.contains_key("purpose"));
        assert!(!obj.contains_key("session_id"));
        assert!(!obj.contains_key("act_chain"));
        assert!(!obj.contains_key("rewrites"));
        assert!(!obj.contains_key("redaction_skip_reason"));
        assert_eq!(
            obj.get("subject_in").and_then(|v| v.as_str()),
            Some("mcp.gateway.request.fs.tools.call")
        );
        assert_eq!(obj.get("outcome").and_then(|v| v.as_str()), Some("allow"));
        assert_eq!(obj.get("identity_source").and_then(|v| v.as_str()), Some("jwt"));
    }

    #[test]
    fn populated_identity_fields_serialize_expected_keys() {
        let identity = IdentityFields {
            agent_id: Some("oncall-agent".to_string()),
            agent_version: Some("1.2.3".to_string()),
            wkl: Some("spiffe://acme.local/ns/prod/sa/oncall".to_string()),
            purpose: Some("incident-response".to_string()),
            session_id: Some("sess_abc".to_string()),
            act_chain: Some(vec![AuditActChainEntry {
                sub: "user:alice".to_string(),
                agent_id: "oncall-agent".to_string(),
                wkl: "spiffe://acme.local/ns/prod/sa/oncall".to_string(),
                iat: 1_715_000_000,
            }]),
        };
        let envelope = AuditEnvelope::new(
            "in".to_string(),
            "out".to_string(),
            "deny",
            "request",
            "tools/call".to_string(),
            None,
            None,
            None,
            IdentitySource::Anonymous,
            None,
            Some(&identity),
        );
        let json = serde_json::to_value(&envelope).expect("serialize");
        let obj = json.as_object().expect("object");
        assert_eq!(obj.get("agent_id").and_then(|v| v.as_str()), Some("oncall-agent"));
        assert_eq!(obj.get("agent_version").and_then(|v| v.as_str()), Some("1.2.3"));
        assert_eq!(
            obj.get("wkl").and_then(|v| v.as_str()),
            Some("spiffe://acme.local/ns/prod/sa/oncall")
        );
        assert_eq!(obj.get("purpose").and_then(|v| v.as_str()), Some("incident-response"));
        assert_eq!(obj.get("session_id").and_then(|v| v.as_str()), Some("sess_abc"));
        let chain = obj
            .get("act_chain")
            .and_then(|v| v.as_array())
            .expect("act_chain array");
        assert_eq!(chain.len(), 1);
        assert_eq!(chain[0].get("sub").and_then(|v| v.as_str()), Some("user:alice"));
        assert_eq!(chain[0].get("iat").and_then(|v| v.as_i64()), Some(1_715_000_000));
    }

    #[test]
    fn act_chain_entry_round_trip_serde() {
        let entry = AuditActChainEntry {
            sub: "user:bob".to_string(),
            agent_id: "agent-b".to_string(),
            wkl: "spiffe://example/wkl".to_string(),
            iat: 1_700_000_000,
        };
        let json = serde_json::to_string(&entry).expect("serialize");
        let restored: AuditActChainEntry = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(restored, entry);
    }

    #[test]
    fn envelope_json_round_trip_is_stable() {
        let identity = IdentityFields {
            agent_id: Some("agent-x".to_string()),
            agent_version: None,
            wkl: Some("spiffe://example/wkl".to_string()),
            purpose: Some("test".to_string()),
            session_id: Some("sess".to_string()),
            act_chain: None,
        };
        let envelope = AuditEnvelope::new(
            "subj.in".to_string(),
            "subj.out".to_string(),
            "allow",
            "response",
            "resources/read".to_string(),
            Some("tenant-1".to_string()),
            Some("sub-1".to_string()),
            None,
            IdentitySource::LegacyHeader,
            Some(serde_json::json!(42)),
            Some(&identity),
        );
        let encoded = serde_json::to_value(&envelope).expect("serialize");
        let wire = serde_json::to_string(&encoded).expect("encode");
        let decoded: serde_json::Value = serde_json::from_str(&wire).expect("decode");
        assert_eq!(decoded, encoded);
        assert_eq!(decoded.get("agent_id").and_then(|v| v.as_str()), Some("agent-x"));
        assert_eq!(decoded.get("purpose").and_then(|v| v.as_str()), Some("test"));
    }
}
