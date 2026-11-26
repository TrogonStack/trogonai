use std::time::Duration;

use trogon_sandbox_contract::{ExecutionId, LeaseId, ReadLogsRequest, ReadLogsResponse};
use trogon_sandbox_protocol::subjects;

use crate::{ClientError, SandboxClient};

impl SandboxClient {
    /// Reads one page of durable command output after the `after_seq` cursor (0 reads from the oldest retained
    /// chunk). Resume with the returned `next_seq`.
    pub async fn read_logs(
        &self,
        lease_id: &LeaseId,
        execution_id: &ExecutionId,
        after_seq: u64,
        max_bytes: Option<u64>,
    ) -> Result<ReadLogsResponse, ClientError> {
        self.read_logs_waiting(lease_id, execution_id, after_seq, max_bytes, None)
            .await
    }

    /// Like [`Self::read_logs`], but when no chunk follows `after_seq` yet the broker holds the reply for up to
    /// `wait` (bounded by [`ReadLogsRequest::MAX_WAIT_MS`]) until one arrives.
    pub async fn read_logs_waiting(
        &self,
        lease_id: &LeaseId,
        execution_id: &ExecutionId,
        after_seq: u64,
        max_bytes: Option<u64>,
        wait: Option<Duration>,
    ) -> Result<ReadLogsResponse, ClientError> {
        let request = ReadLogsRequest {
            lease_id: lease_id.clone(),
            execution_id: execution_id.clone(),
            after_seq,
            max_bytes,
            wait_ms: wait.map(|wait| u64::try_from(wait.as_millis()).unwrap_or(u64::MAX)),
        };
        let timeout = self.default_timeout + wait.unwrap_or_default();
        let response: ReadLogsResponse = self
            .send(&subjects::lease_read_logs(lease_id), &request, timeout)
            .await?;
        match response {
            ReadLogsResponse::Denied(error) => Err(ClientError::Denied(error)),
            other => Ok(other),
        }
    }
}
