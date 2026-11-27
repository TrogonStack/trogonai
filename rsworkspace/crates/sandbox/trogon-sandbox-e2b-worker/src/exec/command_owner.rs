use serde::{Deserialize, Serialize};
use trogon_sandbox_contract::{DispatchAttemptId, LeaseEpoch, LeaseId, TenantId, WorkerId};

use super::command_owner_error::CommandOwnerError;
use super::constants::{MAX_LEASE_IDENTITY_BYTES, MAX_SANDBOX_IDENTITY_BYTES};

/// The assignment a command launch belongs to; custody only answers to the same assignment.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CommandOwner {
    pub(crate) tenant: TenantId,
    pub(crate) lease_id: LeaseId,
    pub(crate) sandbox_id: String,
    pub(crate) worker_id: WorkerId,
    pub(crate) attempt_id: DispatchAttemptId,
    pub(crate) epoch: LeaseEpoch,
}

impl CommandOwner {
    pub(crate) fn validate(&self) -> Result<(), CommandOwnerError> {
        let lease = self.lease_id.as_str();
        if lease.is_empty() || lease.len() > MAX_LEASE_IDENTITY_BYTES || lease.chars().any(char::is_control) {
            return Err(CommandOwnerError::LeaseIdentity);
        }
        if self.sandbox_id.is_empty()
            || self.sandbox_id.len() > MAX_SANDBOX_IDENTITY_BYTES
            || !self
                .sandbox_id
                .bytes()
                .all(|value| value.is_ascii_alphanumeric() || matches!(value, b'-' | b'_'))
        {
            return Err(CommandOwnerError::SandboxIdentity);
        }
        Ok(())
    }
}
