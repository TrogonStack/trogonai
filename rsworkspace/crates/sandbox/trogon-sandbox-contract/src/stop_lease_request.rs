use serde::{Deserialize, Serialize};

use crate::{LeaseId, TenantId};

/// Client-facing request to stop a lease's current session while keeping the
/// sandbox (and any snapshot tagged under its name) fully resumable later.
///
/// A stop releases the sandbox, captures a snapshot when one was requested, and
/// refunds quota. It is the public, tenant-facing verb and checks tenant ownership.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct StopLeaseRequest {
    pub tenant: TenantId,
    pub lease_id: LeaseId,
}
