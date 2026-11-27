use std::collections::BTreeMap;

use serde::Serialize;
use serde_json::Value;

use crate::{Resources, VercelProjectId};

/// Body of runtime (`v2`) and image/snapshot (`v3`) sandbox creation.
#[derive(Debug, Clone, Serialize)]
pub struct NewSandbox {
    #[serde(rename = "projectId")]
    pub project_id: VercelProjectId,
    /// A built in runtime such as `node24`. Sent instead of `image` and `source`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub runtime: Option<String>,
    /// A Vercel managed or custom image reference.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
    /// `{"type":"snapshot","snapshotId":...}` to restore a snapshot.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<Value>,
    /// Sandbox lifetime in milliseconds.
    pub timeout: u64,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub ports: Vec<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resources: Option<Resources>,
    pub persistent: bool,
    #[serde(rename = "networkPolicy", skip_serializing_if = "Option::is_none")]
    pub network_policy: Option<Value>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub region: Option<String>,
    /// At most the contract's tag limit, each value at most 256 characters.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub tags: BTreeMap<String, String>,
}

impl NewSandbox {
    /// A sandbox with only the required parts; the rest is set on the public fields.
    pub fn new(project_id: VercelProjectId, timeout_ms: u64, persistent: bool) -> Self {
        Self {
            project_id,
            runtime: None,
            image: None,
            source: None,
            timeout: timeout_ms,
            ports: Vec::new(),
            resources: None,
            persistent,
            network_policy: None,
            env: BTreeMap::new(),
            region: None,
            tags: BTreeMap::new(),
        }
    }
}
