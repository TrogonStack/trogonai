use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use futures::StreamExt;
use futures::stream::BoxStream;
use trogon_sandbox_client::{EvidenceStream, SandboxClient};
use trogon_sandbox_contract::{ExecutionId, LeaseId, PollCommandResponse, StdinEof, UnixSignal};
use trogon_sandbox_protocol::subjects::evidence_command_suffix;

use crate::client_stdin_sink::ClientStdinSink;
use crate::exit_state::ExitState;
use crate::lock_ext::LockExt;
use crate::lsp_error::LspError;
use crate::lsp_transport::LspTransport;
use crate::output_chunk::OutputChunk;
use crate::output_decoding::decode_chunk;
use crate::signal::Signal;
use crate::stdin_writer::StdinWriter;

pub(crate) struct SandboxTransport {
    pub(crate) stdin: StdinWriter<ClientStdinSink>,
    pub(crate) client: Arc<SandboxClient>,
    pub(crate) lease_id: LeaseId,
    pub(crate) execution_id: ExecutionId,
    pub(crate) evidence: Mutex<Option<EvidenceStream>>,
}

#[async_trait]
impl LspTransport for SandboxTransport {
    async fn write(&self, data: &[u8], eof: StdinEof) -> Result<(), LspError> {
        if eof.closes() && data.is_empty() {
            self.stdin.close().await
        } else {
            self.stdin.write(data, eof).await
        }
    }

    fn output(&self) -> Result<BoxStream<'static, Result<OutputChunk, LspError>>, LspError> {
        let evidence = self
            .evidence
            .locked()
            .take()
            .ok_or_else(|| LspError::Transport("command output was already taken".into()))?;
        let suffix = evidence_command_suffix(&self.execution_id);
        Ok(evidence
            .filter_map(move |frame| {
                let decoded = frame.map_err(LspError::from).and_then(|frame| {
                    (frame.subject_suffix == suffix)
                        .then(|| decode_chunk(&frame.payload))
                        .transpose()
                });
                futures::future::ready(decoded.transpose())
            })
            .boxed())
    }

    async fn poll_exit(&self) -> Result<ExitState, LspError> {
        let response = self.client.poll_command(&self.lease_id, &self.execution_id).await?;
        Ok(match response {
            PollCommandResponse::Running => ExitState::Running,
            PollCommandResponse::Completed(output) => ExitState::Exited(Some(output.exit_code)),
            PollCommandResponse::NotFound | PollCommandResponse::Denied(_) => ExitState::Exited(None),
        })
    }

    async fn kill(&self, signal: Signal) -> Result<(), LspError> {
        let signal = match signal {
            Signal::Term => UnixSignal::Term,
            Signal::Kill => UnixSignal::Kill,
        };
        self.client
            .kill_command(&self.lease_id, &self.execution_id, signal)
            .await?;
        Ok(())
    }
}
