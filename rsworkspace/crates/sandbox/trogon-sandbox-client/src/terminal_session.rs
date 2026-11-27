use futures::StreamExt;
use futures::stream::BoxStream;
use trogon_sandbox_contract::{LeaseId, TerminalId, TerminalOp, TerminalOutput, TerminalRequest, TerminalResponse};
use trogon_sandbox_protocol::subjects;

use crate::constants::TERMINAL_TIMEOUT;
use crate::{ClientError, SandboxClient};

impl SandboxClient {
    /// Sends one terminal operation for `terminal_id` on the lease. Subscribe with [`Self::terminal_output`] before
    /// sending `Open` so the first prompt is not missed. A `Denied` reply becomes `ClientError::Denied`; `Failed`
    /// and `Unsupported` are returned for the caller to inspect.
    pub async fn terminal(
        &self,
        lease_id: &LeaseId,
        terminal_id: TerminalId,
        op: TerminalOp,
    ) -> Result<TerminalResponse, ClientError> {
        let request = TerminalRequest {
            lease_id: lease_id.clone(),
            terminal_id,
            op,
        };
        let response: TerminalResponse = self
            .send(&subjects::lease_terminal(lease_id), &request, TERMINAL_TIMEOUT)
            .await?;
        match response {
            TerminalResponse::Denied(error) => Err(ClientError::Denied(error)),
            other => Ok(other),
        }
    }

    /// Subscribes to a terminal's output frames. Frames that do not decode are skipped. The stream is live only:
    /// nothing is replayed, and a gap in `seq` means this reader was too slow and lost frames.
    pub async fn terminal_output(
        &self,
        terminal_id: &TerminalId,
    ) -> Result<BoxStream<'static, TerminalOutput>, ClientError> {
        let raw = self.bus.subscribe(&subjects::terminal_output(terminal_id)).await?;
        Ok(raw
            .filter_map(|message| async move { serde_json::from_slice::<TerminalOutput>(&message.payload).ok() })
            .boxed())
    }
}
