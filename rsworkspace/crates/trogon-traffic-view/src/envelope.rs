use sha2::{Digest, Sha256};

use crate::error::ProjectorError;

/// Parsed JetStream audit payload plus routing subject.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuditEnvelope {
    pub event_id: String,
    pub subject: String,
    pub payload: serde_json::Value,
}

impl AuditEnvelope {
    pub fn from_message(subject: &str, payload: &[u8]) -> Result<Self, ProjectorError> {
        let json: serde_json::Value =
            serde_json::from_slice(payload).map_err(|error| ProjectorError::Decode(error.to_string()))?;
        let event_id = json
            .get("event_id")
            .and_then(|value| value.as_str())
            .map(str::to_owned)
            .unwrap_or_else(|| stable_event_id(subject, payload));
        Ok(Self {
            event_id,
            subject: subject.to_owned(),
            payload: json,
        })
    }
}

pub fn stable_event_id(subject: &str, payload: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(subject.as_bytes());
    hasher.update(payload);
    format!("{:x}", hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stable_event_id_is_deterministic() {
        let a = stable_event_id("mcp.audit.sts.success", br#"{"outcome":"success"}"#);
        let b = stable_event_id("mcp.audit.sts.success", br#"{"outcome":"success"}"#);
        assert_eq!(a, b);
    }

    #[test]
    fn envelope_honors_embedded_event_id() {
        let envelope = AuditEnvelope::from_message(
            "mcp.audit.sts.success",
            br#"{"event_id":"evt-1","outcome":"success"}"#,
        )
        .expect("parse");
        assert_eq!(envelope.event_id, "evt-1");
    }
}
