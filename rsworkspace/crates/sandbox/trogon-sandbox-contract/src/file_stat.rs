use serde::{Deserialize, Serialize};

use crate::FileKind;

/// Metadata for one filesystem entry.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct FileStat {
    /// Path as the caller addresses it, relative to the request's base path for listings.
    pub path: String,
    pub name: String,
    pub kind: FileKind,
    /// Size in bytes. Zero for directories on providers that do not report one.
    pub size: u64,
    /// Unix permission bits, when the provider reports them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<u32>,
    /// Modification time as milliseconds since the Unix epoch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub modified_unix_ms: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub symlink_target: Option<String>,
}
