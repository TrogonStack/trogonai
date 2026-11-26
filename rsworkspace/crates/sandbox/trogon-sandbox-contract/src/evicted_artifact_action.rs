use serde::{Deserialize, Serialize};

/// What the retention sweep does with a snapshot artifact it evicts.
///
/// It serializes as a JSON boolean, `true` for `Delete`, so existing peers keep parsing it. `Keep`
/// leaves the physical artifact, and its storage charge, in the registry.
#[derive(Debug, Clone, Copy, Default, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(from = "bool", into = "bool")]
pub enum EvictedArtifactAction {
    Delete,
    #[default]
    Keep,
}

impl EvictedArtifactAction {
    pub const fn deletes(self) -> bool {
        matches!(self, Self::Delete)
    }
}

impl From<bool> for EvictedArtifactAction {
    fn from(delete: bool) -> Self {
        if delete { Self::Delete } else { Self::Keep }
    }
}

impl From<EvictedArtifactAction> for bool {
    fn from(action: EvictedArtifactAction) -> Self {
        action.deletes()
    }
}
