use serde::{Deserialize, Serialize};

use crate::{OciArtifactRef, WorkerId};

/// Where the state of a suspended lease is kept, when the provider does not
/// keep it under its own sandbox id.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum DormancyArtifact {
    /// On the worker that suspended the lease, found again by `id`.
    Local { worker_id: WorkerId, id: String },
    /// In a registry, so any worker can wake the lease.
    Remote(OciArtifactRef),
}
