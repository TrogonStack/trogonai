use serde::{Deserialize, Serialize};

/// Lifecycle state for a registered agent definition.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LifecycleState {
    Active,
    Deprecated,
    Revoked,
}

/// Authoritative agent identity record stored in NATS KV.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentRecord {
    pub agent_id: String,
    pub agent_version: String,
    pub agent_definition_digest: String,
    pub owner_team: String,
    pub allowed_workloads: Vec<String>,
    pub allowed_tools: Vec<String>,
    pub allowed_audiences: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allowed_purposes: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mesh_token_ttl_s: Option<u32>,
    #[serde(default)]
    pub metadata: serde_json::Value,
    pub lifecycle_state: LifecycleState,
    pub created_at: String,
    pub updated_at: String,
}

/// Runtime lookup request on `mcp.registry.agent.lookup`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LookupRequest {
    pub agent_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tenant_hint: Option<String>,
}

/// Lookup reply variants returned to callers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
#[allow(clippy::large_enum_variant)]
pub enum LookupResponse {
    Found { record: AgentRecord },
    NotFound,
    Revoked { reason: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_record() -> AgentRecord {
        AgentRecord {
            agent_id: "acme/oncall-agent".to_string(),
            agent_version: "3.2.1".to_string(),
            agent_definition_digest: "sha256:abc123".to_string(),
            owner_team: "platform-sre".to_string(),
            allowed_workloads: vec!["spiffe://acme.local/ns/prod/sa/oncall-agent".to_string()],
            allowed_tools: vec!["pagerduty.page".to_string()],
            allowed_audiences: vec!["mcp.server.pagerduty".to_string()],
            allowed_purposes: Some(vec!["incident.response".to_string()]),
            mesh_token_ttl_s: Some(300),
            metadata: serde_json::json!({"description": "Pages on-call"}),
            lifecycle_state: LifecycleState::Active,
            created_at: "2026-05-27T00:00:00Z".to_string(),
            updated_at: "2026-05-27T12:00:00Z".to_string(),
        }
    }

    #[test]
    fn agent_record_serde_round_trip() {
        let record = sample_record();
        let encoded = serde_json::to_string(&record).expect("serialize");
        let decoded: AgentRecord = serde_json::from_str(&encoded).expect("deserialize");
        assert_eq!(decoded, record);
    }

    #[test]
    fn lookup_response_variants_serialize() {
        let found = LookupResponse::Found {
            record: sample_record(),
        };
        let json = serde_json::to_value(&found).expect("serialize found");
        assert_eq!(json.get("status").and_then(|v| v.as_str()), Some("found"));

        let not_found = LookupResponse::NotFound;
        let json = serde_json::to_value(&not_found).expect("serialize not_found");
        assert_eq!(json.get("status").and_then(|v| v.as_str()), Some("not_found"));

        let revoked = LookupResponse::Revoked {
            reason: "policy".to_string(),
        };
        let json = serde_json::to_value(&revoked).expect("serialize revoked");
        assert_eq!(json.get("status").and_then(|v| v.as_str()), Some("revoked"));
    }
}
