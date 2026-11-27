use serde::Deserialize;

/// One run of a sandbox.
#[derive(Debug, Clone, Eq, PartialEq, Deserialize)]
pub struct SessionRecord {
    pub id: String,
    /// `pending`, `running`, `stopping`, `stopped`, `failed`, `aborted` or `snapshotting`.
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub vcpus: Option<u32>,
    #[serde(default)]
    pub memory: Option<u32>,
    #[serde(default)]
    pub region: Option<String>,
}
