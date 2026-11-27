use serde::{Deserialize, Serialize};
use trogon_sandbox_contract::{LeaseId, TenantId, WorkerId};

use crate::{BoundedName, CreateIntent, CustodyPhase, CustodyToken, LeaseResources};

/// What the worker durably knows about one lease's cluster resources.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CustodyRecord {
    pub(crate) version: u32,
    pub lease_id: LeaseId,
    pub(crate) worker: WorkerId,
    pub(crate) namespace: BoundedName,
    pub(crate) generation: CustodyToken,
    pub tenant: TenantId,
    pub phase: CustodyPhase,
    pub resources: LeaseResources,
    pub create: Option<CreateIntent>,
}

impl CustodyRecord {
    /// Whether the record names a complete VM and root volume and nothing is in flight.
    pub(crate) fn ready_identity(&self) -> bool {
        self.create.is_none()
            && self.resources.pending_claim.is_none()
            && self.resources.vm.is_some()
            && self.resources.data_volume.is_some()
    }
}
