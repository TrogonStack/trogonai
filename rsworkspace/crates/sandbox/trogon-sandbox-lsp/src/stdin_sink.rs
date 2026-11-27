use async_trait::async_trait;
use trogon_sandbox_client::ClientError;
use trogon_sandbox_contract::{StdinChunk, StdinEof, StdinSeq, WriteStdinResponse};

/// One `write_stdin` call against the sandbox, with no retry policy of its own.
#[async_trait]
pub trait StdinSink: Send + Sync {
    async fn send(&self, seq: StdinSeq, data: StdinChunk, eof: StdinEof) -> Result<WriteStdinResponse, ClientError>;
}
