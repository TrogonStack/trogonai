use serde::{Deserialize, Serialize};

use crate::LeaseId;

/// Client-facing request to extend the wall-clock deadline of an already-running lease without restarting it.
///
/// The broker rejects the request once the lease's total lifetime, counted from creation, would exceed its configured
/// cap. The default cap is 86400 seconds, matching a paid-tier sandbox ceiling, and an operator may raise it.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct ExtendLeaseTimeoutRequest {
    pub lease_id: LeaseId,
    pub additional_seconds: u32,
}
