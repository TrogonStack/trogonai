use serde::{Deserialize, Serialize};

use crate::LeaseId;

/// Client-facing request for the vendor's own handle on a running lease, so a caller can reach the sandbox with the
/// vendor SDK.
///
/// Tenant policy gates it the same way as asking for vendor passthrough when a lease is requested.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct NativeHandleRequest {
    pub lease_id: LeaseId,
}
