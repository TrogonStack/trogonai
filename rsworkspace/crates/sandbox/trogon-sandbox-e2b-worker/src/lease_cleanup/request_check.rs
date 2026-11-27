use trogon_sandbox_contract::CancelOwnedLeaseRequest;

use super::constants::MAX_LEASE_ID_BYTES;
use super::lease_cleanup_error::LeaseCleanupError;
use crate::native_sandbox_id::NativeSandboxId;

/// Refuses a cleanup request whose lease or sandbox identity is not bounded and canonical.
pub(crate) fn validate_request(request: &CancelOwnedLeaseRequest) -> Result<(), LeaseCleanupError> {
    let lease = request.lease_id.as_str();
    if lease.is_empty() || lease.len() > MAX_LEASE_ID_BYTES || lease.chars().any(char::is_control) {
        return Err(LeaseCleanupError::UnboundedOwner);
    }
    NativeSandboxId::new(request.sandbox_id.as_str())?;
    Ok(())
}
