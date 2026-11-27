//! Helpers shared by the command tests.

#![allow(dead_code)]

use trogon_sandbox_client::ClientError;
use trogon_sandbox_contract::{ExecutionId, LeaseError, LeaseId};

pub fn execution_id() -> ExecutionId {
    ExecutionId::new("exec-1")
}

/// A denial that names the lease as not running.
pub fn not_running() -> LeaseError {
    LeaseError::LeaseNotRunning {
        lease_id: LeaseId::new("lease-1"),
    }
}

/// True when `result` failed with the broker's not-running denial.
pub fn is_not_running<T>(result: &Result<T, ClientError>) -> bool {
    matches!(result, Err(ClientError::Denied(LeaseError::LeaseNotRunning { .. })))
}
