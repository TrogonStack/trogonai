use std::collections::BTreeMap;
use std::time::SystemTime;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub(crate) struct LeaseIdInput {
    pub lease_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub(crate) struct WaitReleasedInput {
    pub lease_id: String,
    pub timeout_seconds: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub(crate) struct ListCapabilitiesInput {
    pub provider: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub(crate) struct ListLeasesInput {
    pub status_filter: Option<String>,
    pub cursor: Option<String>,
    pub limit: Option<u32>,
    /// Keep only leases whose tags contain every entry.
    #[serde(default)]
    pub tags: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub(crate) struct ReapLeasesInput {
    /// Release only leases whose tags contain every entry.
    #[serde(default)]
    pub tags: BTreeMap<String, String>,
    /// Release only leases created at least this many seconds ago.
    pub older_than_seconds: Option<u32>,
    /// Release only leases with no activity for at least this many seconds.
    pub idle_for_seconds: Option<u32>,
    /// Report what would be released without releasing anything.
    #[serde(default)]
    pub dry_run: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub(crate) struct SubscribeEvidenceOutput {
    pub subscribed: bool,
    pub lease_id: String,
    pub evidence_subject: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub(crate) struct EvidenceNotification {
    pub lease_id: String,
    pub subject_suffix: String,
    pub payload_base64: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub(crate) struct ExtendTimeoutInput {
    pub lease_id: String,
    pub additional_seconds: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub(crate) struct ExtendTimeoutOutput {
    pub new_deadline: SystemTime,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub(crate) struct LimitsInput {}
