use serde::Deserialize;
use serde_json::Value;

use crate::{SandboxState, SpaceId};

/// A sandbox as SPACE describes it.
#[derive(Debug, Clone, Deserialize)]
pub struct SandboxInfo {
    /// The sandbox identifier.
    pub id: SpaceId,
    /// The sandbox's lifecycle state.
    pub state: SandboxState,
    /// The failure code SPACE reports for a failed sandbox.
    #[serde(default)]
    pub failure_code: Option<String>,
    /// The metadata the sandbox was created with.
    #[serde(default)]
    pub metadata: Value,
}

impl SandboxInfo {
    /// A metadata entry, when it is text.
    pub fn metadata_text(&self, key: &str) -> Option<&str> {
        self.metadata[key].as_str()
    }
}
