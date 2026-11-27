use async_trait::async_trait;
use trogon_sandbox_contract::{WorkerAuthorityRequest, WorkerAuthorityResponse, WorkerId};

use crate::subjects::worker_authority_check;
use crate::{AuthorityOracle, OracleError};

/// Asks the broker over NATS request-reply.
pub struct NatsAuthorityOracle {
    client: async_nats::Client,
    worker_id: WorkerId,
}

impl NatsAuthorityOracle {
    /// An oracle that asks on `worker_id`'s authority subject through `client`.
    pub fn new(client: async_nats::Client, worker_id: WorkerId) -> Self {
        Self { client, worker_id }
    }
}

#[async_trait]
impl AuthorityOracle for NatsAuthorityOracle {
    async fn owned(&self, request: &WorkerAuthorityRequest) -> Result<WorkerAuthorityResponse, OracleError> {
        let body = serde_json::to_vec(request).map_err(OracleError::Encode)?;
        let reply = self
            .client
            .request(worker_authority_check(&self.worker_id), body.into())
            .await
            .map_err(|error| match error.kind() {
                async_nats::client::RequestErrorKind::NoResponders => OracleError::NoResponders,
                _ => OracleError::Request(error),
            })?;
        serde_json::from_slice(&reply.payload).map_err(OracleError::Decode)
    }
}
