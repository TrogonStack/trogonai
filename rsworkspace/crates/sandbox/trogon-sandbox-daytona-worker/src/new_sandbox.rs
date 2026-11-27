use std::collections::BTreeMap;

use serde::Serialize;

/// Body of a sandbox create call. A sandbox starts from a named snapshot, which fixes its own size.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NewSandbox {
    pub snapshot: String,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub labels: BTreeMap<String, String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub public: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    /// Minutes of inactivity before the sandbox stops; zero never stops it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auto_stop_interval: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auto_archive_interval: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auto_delete_interval: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub network_block_all: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub network_allow_list: Option<String>,
}

impl NewSandbox {
    pub fn new(snapshot: impl Into<String>) -> Self {
        Self {
            snapshot: snapshot.into(),
            ..Self::default()
        }
    }
}
