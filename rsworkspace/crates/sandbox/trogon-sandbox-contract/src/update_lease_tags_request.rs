use serde::{Deserialize, Serialize};

use crate::{LeaseId, SandboxTags};

/// Request to replace the full `tags` set of an already-running lease.
///
/// The provided set replaces the existing one entirely. It is not merged.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct UpdateLeaseTagsRequest {
    pub lease_id: LeaseId,
    pub tags: SandboxTags,
}
