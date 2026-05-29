use kube::CustomResource;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Declarative MCP gateway configuration for a tenant namespace.
#[derive(CustomResource, Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[kube(
    group = "gateway.trogon.ai",
    version = "v1alpha1",
    kind = "MCPGatewayConfig",
    plural = "mcpgatewayconfigs",
    shortname = "mgw",
    namespaced,
    status = "MCPGatewayConfigStatus",
    printcolumn = r#"{"name":"Tenant","type":"string","jsonPath":".spec.tenantId"}"#,
    printcolumn = r#"{"name":"Bundle","type":"string","jsonPath":".spec.bundleRef"}"#,
    printcolumn = r#"{"name":"Ready","type":"string","jsonPath":".status.conditions[?(@.type==\"Ready\")].status"}"#,
    printcolumn = r#"{"name":"Age","type":"date","jsonPath":".metadata.creationTimestamp"}"#
)]
#[serde(rename_all = "camelCase")]
pub struct MCPGatewayConfigSpec {
    /// OCI or KV bundle reference (`tenant/slug` per gateway manifest scope).
    pub bundle_ref: String,
    /// Tenant identifier copied into projected KV JSON.
    pub tenant_id: String,
    /// Redaction policy identifiers applied at ingress (stub list until WIT policies land).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub redaction_policies: Vec<String>,
    /// Rate limit knobs keyed by scope (`caller`, `server`, `tenant`, …).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub rate_limits: BTreeMap<String, u32>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MCPGatewayConfigStatus {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed_generation: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_synced_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nats_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kv_revision: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub conditions: Vec<MCPGatewayConfigCondition>,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MCPGatewayConfigCondition {
    #[serde(rename = "type")]
    pub condition_type: String,
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_transition_time: Option<String>,
}
