use serde::{Deserialize, Serialize};

use crate::{EntityStateKind, LeaseId};

/// Where an entity is in its life. A state that runs the entity names the lease that runs it.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntityState {
    Creating,
    Starting { lease: LeaseId },
    Running { lease: LeaseId },
    Paused { lease: LeaseId },
    Stopping { lease: LeaseId },
    Stopped,
    Deleting,
    Failed { reason: String },
}

impl EntityState {
    /// The state without its payload.
    pub fn kind(&self) -> EntityStateKind {
        match self {
            Self::Creating => EntityStateKind::Creating,
            Self::Starting { .. } => EntityStateKind::Starting,
            Self::Running { .. } => EntityStateKind::Running,
            Self::Paused { .. } => EntityStateKind::Paused,
            Self::Stopping { .. } => EntityStateKind::Stopping,
            Self::Stopped => EntityStateKind::Stopped,
            Self::Deleting => EntityStateKind::Deleting,
            Self::Failed { .. } => EntityStateKind::Failed,
        }
    }

    /// The lease this state is bound to, if any.
    pub fn lease(&self) -> Option<&LeaseId> {
        match self {
            Self::Starting { lease } | Self::Running { lease } | Self::Paused { lease } | Self::Stopping { lease } => {
                Some(lease)
            }
            Self::Creating | Self::Stopped | Self::Deleting | Self::Failed { .. } => None,
        }
    }
}
