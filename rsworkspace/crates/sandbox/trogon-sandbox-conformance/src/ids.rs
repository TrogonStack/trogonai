use trogon_sandbox_contract::{DispatchAttemptId, LeaseId, LeaseRequestKey, TenantId};

use crate::CheckFailureError;
use crate::constants::TENANT;

pub(crate) fn fresh() -> String {
    uuid::Uuid::now_v7().to_string()
}

pub(crate) fn fresh_lease_id() -> LeaseId {
    LeaseId::new(fresh())
}

pub(crate) fn fresh_attempt_id() -> Result<DispatchAttemptId, CheckFailureError> {
    DispatchAttemptId::new(fresh()).map_err(CheckFailureError::identifier)
}

pub(crate) fn fresh_request_key() -> Result<LeaseRequestKey, CheckFailureError> {
    LeaseRequestKey::new(fresh()).map_err(CheckFailureError::identifier)
}

pub(crate) fn tenant() -> Result<TenantId, CheckFailureError> {
    TenantId::new(TENANT).map_err(CheckFailureError::identifier)
}

pub(crate) fn fresh_execution_id() -> trogon_sandbox_contract::ExecutionId {
    trogon_sandbox_contract::ExecutionId::new(fresh())
}

#[cfg(test)]
mod tests;
