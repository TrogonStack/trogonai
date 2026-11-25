use serde::{Deserialize, Serialize};

/// A single file to write into an already-running lease's sandbox filesystem.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct SandboxFileWrite {
    pub path: String,
    pub content: Vec<u8>,
    /// Unix file mode, such as `0o644`. Left to the provider's default when unset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<u32>,
}
