use serde::{Deserialize, Serialize};
use trogon_sandbox_contract::{CancelOwnedLeaseRequest, LeaseId, TenantId};

use super::constants::RECEIPT_VERSION;
use super::lease_cleanup_error::LeaseCleanupError;
use super::phase::Phase;
use super::request_check::validate_request;
use crate::native_sandbox_id::NativeSandboxId;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Receipt {
    version: u32,
    tenant: TenantId,
    lease_id: LeaseId,
    sandbox_id: NativeSandboxId,
    pub phase: Phase,
    pub claim_until_unix_ms: u64,
}

impl Receipt {
    pub(super) fn pending(request: &CancelOwnedLeaseRequest) -> Result<Self, LeaseCleanupError> {
        validate_request(request)?;
        Ok(Self {
            version: RECEIPT_VERSION,
            tenant: request.tenant.clone(),
            lease_id: request.lease_id.clone(),
            sandbox_id: NativeSandboxId::new(request.sandbox_id.as_str())?,
            phase: Phase::Cleanup,
            claim_until_unix_ms: 0,
        })
    }

    pub(super) fn validate(&self, request: &CancelOwnedLeaseRequest) -> Result<(), LeaseCleanupError> {
        validate_request(request)?;
        if self.version != RECEIPT_VERSION
            || self.tenant != request.tenant
            || self.lease_id != request.lease_id
            || self.sandbox_id.as_str() != request.sandbox_id.as_str()
        {
            return Err(LeaseCleanupError::OwnerMismatch);
        }
        if self.phase == Phase::Absent && self.claim_until_unix_ms != 0 {
            return Err(LeaseCleanupError::InvalidClaim);
        }
        Ok(())
    }
}
