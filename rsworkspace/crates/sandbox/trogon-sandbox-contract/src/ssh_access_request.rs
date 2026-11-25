use serde::{Deserialize, Serialize};

use crate::LeaseId;

/// Client-facing request for direct SSH access to a running lease.
///
/// It serves providers that offer SSH from their own gateway rather than through a stream the SSH gateway can
/// tunnel.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct SshAccessRequest {
    pub lease_id: LeaseId,
    /// How long the access should stay valid. `None` takes the provider's default; providers round it to their own
    /// granularity.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ttl_seconds: Option<u32>,
}
