use serde::{Deserialize, Serialize};

use crate::{CorrelationId, DispatchTarget, DriveRecord, LeaseId, LeaseRequest};

/// The payload the broker sends to a provider worker to launch a lease.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct ProviderDispatchPayload {
    pub lease_id: LeaseId,
    pub request: LeaseRequest,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub correlation_id: Option<CorrelationId>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub resolved_drives: Vec<DriveRecord>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<DispatchTarget>,
}
