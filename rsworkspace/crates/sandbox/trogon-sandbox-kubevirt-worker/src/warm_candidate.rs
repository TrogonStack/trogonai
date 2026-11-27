use serde::{Deserialize, Serialize};
use trogon_sandbox_contract::Digest;

use crate::ResourceReceipt;

/// An idle warm VM and its root volume, pinned at the resource version it was observed at.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WarmCandidate {
    pub vm: ResourceReceipt,
    pub data_volume: ResourceReceipt,
    pub resource_version: String,
    pub digest: Digest,
}
