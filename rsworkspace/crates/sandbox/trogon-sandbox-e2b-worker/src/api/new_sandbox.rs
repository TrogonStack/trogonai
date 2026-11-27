use std::collections::BTreeMap;

use serde::Serialize;

use super::AutoPause;
use crate::network::{InternetAccess, NetworkRules};

/// Body of `POST /v2/sandboxes`.
#[derive(Debug, Clone, Default, Serialize)]
pub struct NewSandbox {
    #[serde(rename = "templateID")]
    pub template_id: String,
    /// Sandbox lifetime in seconds.
    pub timeout: u32,
    #[serde(rename = "autoPause", skip_serializing_if = "Option::is_none")]
    pub auto_pause: Option<AutoPause>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allow_internet_access: Option<InternetAccess>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub network: Option<NetworkRules>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub metadata: BTreeMap<String, String>,
    #[serde(rename = "envVars", skip_serializing_if = "BTreeMap::is_empty")]
    pub env_vars: BTreeMap<String, String>,
}
