use std::time::SystemTime;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub(crate) struct PauseLeaseInput {
    pub lease_id: String,
    /// Upper bound on how long the lease may stay paused before the sweeper reaps it. Defaults to one hour.
    #[serde(default)]
    pub max_paused_seconds: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub(crate) struct SnapshotDeleteInput {
    pub digest: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub(crate) struct SuspendLeaseInput {
    pub lease_id: String,
    /// `auto` (the default) takes whatever the provider offers; `memory` or `disk` must match it exactly.
    #[serde(default)]
    pub level: Option<String>,
    /// Upper bound on how long the lease may stay suspended before the sweeper reaps it. Defaults to 24 hours.
    #[serde(default)]
    pub max_suspended_seconds: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub(crate) struct SuspendLeaseOutput {
    /// Always `suspended`; a refusal is an error.
    pub outcome: String,
    /// `memory` or `disk`.
    pub level: String,
    /// When the suspended lease is reaped if nobody wakes it.
    pub reap_at: SystemTime,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub(crate) struct WakeLeaseOutput {
    /// Always `woken`; a refusal is an error.
    pub outcome: String,
    pub deadline: SystemTime,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub(crate) struct PauseLeaseOutput {
    /// `paused` or `unsupported`.
    pub outcome: String,
    /// When a paused lease is reaped if nobody resumes it.
    pub reap_at: Option<SystemTime>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub(crate) struct ResumeLeaseOutput {
    /// `resumed` or `unsupported`.
    pub outcome: String,
    pub deadline: Option<SystemTime>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub(crate) struct NetworkPolicyUpdateInput {
    pub lease_id: String,
    /// A `trogon_sandbox_contract::NetworkPolicy` value, e.g. `"allow_all"`, `"deny_all"`, or
    /// `{"user_defined": {"rules": [], "allow_ranges": [], "deny_ranges": []}}`.
    pub network_policy: serde_json::Value,
}
