use sha2::{Digest, Sha256};

use crate::agent_id::A2aAgentId;

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
