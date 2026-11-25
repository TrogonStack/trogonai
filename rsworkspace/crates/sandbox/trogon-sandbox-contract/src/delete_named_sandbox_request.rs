use serde::{Deserialize, Serialize};

use crate::TenantId;

/// Client-facing request to permanently delete a sandbox by its tenant-scoped `name`, even when it has no active lease.
///
/// Deleting a lease only reaches a lease that is still running. A stopped named persistent sandbox survives as a
/// suspended actor with nothing but its name left to find it by, so this request looks it up by name instead and
/// purges whatever it finds: an active lease under that name, a resumable snapshot tag, and the suspended actor
/// itself.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct DeleteNamedSandboxRequest {
    pub tenant: TenantId,
    pub name: String,
}
