use sha2::{Digest, Sha256};

use crate::agent_id::A2aAgentId;

#[derive(Debug, Clone, serde::Serialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum AuditOutcome {
    Ok,
    Err { code: i32, message: String },
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
}

impl AuditEnvelope {
    pub fn new(
        agent_id: &A2aAgentId,
        method: impl Into<String>,
        req_id: Option<String>,
        started_at: u64,
        latency_ms: u64,
        outcome: AuditOutcome,
        raw_params: Option<&[u8]>,
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
        let env = AuditEnvelope::new(&agent(), "message/send", Some("r1".into()), 1000, 5, AuditOutcome::Ok, None);
        let v = serde_json::to_value(&env).unwrap();
        assert_eq!(v["outcome"], "ok");
        assert_eq!(v["agent_id"], "test-agent");
        assert_eq!(v["method"], "message/send");
        assert_eq!(v["req_id"], "r1");
        assert_eq!(v["latency_ms"], 5);
        assert!(v["params_fingerprint"].is_null());
    }

    #[test]
    fn err_outcome_serializes_correctly() {
        let env = AuditEnvelope::new(
            &agent(),
            "tasks/get",
            None,
            2000,
            10,
            AuditOutcome::Err { code: -32001, message: "not found".into() },
            Some(b"some params"),
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
        let env = AuditEnvelope::new(&agent(), "tasks/get", None, 0, 0, AuditOutcome::Ok, Some(b""));
        assert!(env.params_fingerprint.is_none());
    }

    #[test]
    fn params_fingerprint_is_deterministic() {
        let a = AuditEnvelope::new(&agent(), "m", None, 0, 0, AuditOutcome::Ok, Some(b"hello"));
        let b = AuditEnvelope::new(&agent(), "m", None, 0, 0, AuditOutcome::Ok, Some(b"hello"));
        assert_eq!(a.params_fingerprint, b.params_fingerprint);
    }

    #[test]
    fn params_fingerprint_differs_for_different_params() {
        let a = AuditEnvelope::new(&agent(), "m", None, 0, 0, AuditOutcome::Ok, Some(b"hello"));
        let b = AuditEnvelope::new(&agent(), "m", None, 0, 0, AuditOutcome::Ok, Some(b"world"));
        assert_ne!(a.params_fingerprint, b.params_fingerprint);
    }
}
