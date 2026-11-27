use async_trait::async_trait;
use trogon_sandbox_contract::{WorkerAuthorityRequest, WorkerAuthorityResponse};

use crate::OracleError;

/// Asks the broker which leases are still the worker's.
#[async_trait]
pub trait AuthorityOracle: Send + Sync {
    /// Which of the leases in `request` the broker still assigns to the asking worker.
    async fn owned(&self, request: &WorkerAuthorityRequest) -> Result<WorkerAuthorityResponse, OracleError>;
}
