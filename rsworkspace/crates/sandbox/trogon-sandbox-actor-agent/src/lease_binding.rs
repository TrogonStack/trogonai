use std::collections::BTreeMap;
use std::num::NonZeroU16;

use trogon_sandbox_contract::LeaseId;

use crate::BindLeaseRequest;

/// What the bind endpoint installed: the lease this actor currently serves and the per-lease context the actor
/// template could not carry.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct LeaseBinding {
    pub lease_id: LeaseId,
    pub env: BTreeMap<String, String>,
    pub working_dir: Option<String>,
    pub exposed_port: Option<NonZeroU16>,
}

impl From<BindLeaseRequest> for LeaseBinding {
    fn from(request: BindLeaseRequest) -> Self {
        Self {
            lease_id: request.lease_id,
            env: request.env,
            working_dir: request.working_dir,
            exposed_port: request.exposed_port,
        }
    }
}
