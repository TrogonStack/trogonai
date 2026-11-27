use trogon_sandbox_contract::{LeaseError, LeaseId};

use crate::ProxmoxProviderError;

/// How a failed lease operation is reported to the caller.
pub(crate) fn denial(lease: &LeaseId, error: &ProxmoxProviderError) -> LeaseError {
    match error {
        ProxmoxProviderError::NotHeld => LeaseError::LeaseNotFound {
            lease_id: lease.clone(),
        },
        other => LeaseError::Backend {
            message: other.to_string(),
        },
    }
}
