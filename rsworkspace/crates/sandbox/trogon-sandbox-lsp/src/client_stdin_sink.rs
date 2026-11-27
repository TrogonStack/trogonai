use std::sync::Arc;

use async_trait::async_trait;
use trogon_sandbox_client::{ClientError, SandboxClient};
use trogon_sandbox_contract::{ExecutionId, LeaseId, StdinChunk, StdinEof, StdinSeq, WriteStdinResponse};

use crate::stdin_sink::StdinSink;

pub(crate) struct ClientStdinSink {
    pub(crate) client: Arc<SandboxClient>,
    pub(crate) lease_id: LeaseId,
    pub(crate) execution_id: ExecutionId,
}

#[async_trait]
impl StdinSink for ClientStdinSink {
    async fn send(&self, seq: StdinSeq, data: StdinChunk, eof: StdinEof) -> Result<WriteStdinResponse, ClientError> {
        self.client
            .write_stdin(&self.lease_id, &self.execution_id, seq, data, eof)
            .await
    }
}
