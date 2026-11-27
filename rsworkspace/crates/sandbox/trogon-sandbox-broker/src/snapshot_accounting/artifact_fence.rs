use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(super) struct ArtifactFence {
    pub(super) closed_through: u64,
    pub(super) pending_delete: Option<String>,
}
