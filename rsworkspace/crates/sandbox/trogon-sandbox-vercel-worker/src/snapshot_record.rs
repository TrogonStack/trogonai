use serde::Deserialize;

/// The `snapshot` object Vercel returns when a snapshot is created.
#[derive(Debug, Clone, Eq, PartialEq, Deserialize)]
pub struct SnapshotRecord {
    pub id: String,
    #[serde(default, rename = "sizeBytes")]
    pub size_bytes: Option<u64>,
    /// Milliseconds since the epoch; absent when the snapshot never expires.
    #[serde(default, rename = "expiresAt")]
    pub expires_at: Option<u64>,
    #[serde(default)]
    pub status: String,
}
