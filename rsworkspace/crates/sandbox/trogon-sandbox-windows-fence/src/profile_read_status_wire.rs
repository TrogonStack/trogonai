use serde::Deserialize;

/// The version of `fence-helper profile status`'s record format this crate reads.
/// Any other version is not read: its lists decide what may be granted.
pub(crate) const RECORD_VERSION: u64 = 1;

#[derive(Deserialize)]
pub(crate) struct ProfileReadStatusWire {
    pub(crate) state: Option<String>,
    pub(crate) reason: Option<String>,
    pub(crate) record_version: Option<serde_json::Value>,
    pub(crate) exclusions: Option<serde_json::Value>,
    pub(crate) ungranted_only: Option<serde_json::Value>,
    pub(crate) profile: Option<String>,
    pub(crate) capabilities: Option<serde_json::Value>,
    pub(crate) not_granted: Option<serde_json::Value>,
    pub(crate) self_closed: Option<serde_json::Value>,
    pub(crate) unverifiable: Option<serde_json::Value>,
    pub(crate) unprotected: Option<serde_json::Value>,
    pub(crate) stale_allow: Option<serde_json::Value>,
    pub(crate) replaced_files: Option<serde_json::Value>,
    pub(crate) extra_exclusions: Option<serde_json::Value>,
    pub(crate) containers: Option<u64>,
    pub(crate) granted: Option<u64>,
    pub(crate) files: Option<u64>,
    pub(crate) denied: Option<u64>,
    pub(crate) env: Option<serde_json::Map<String, serde_json::Value>>,
    pub(crate) paths: Option<Vec<serde_json::Value>>,
}
