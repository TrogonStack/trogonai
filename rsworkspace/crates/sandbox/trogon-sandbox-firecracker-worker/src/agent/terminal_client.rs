//! The terminal half of the guest-agent client: one long-lived vsock HTTP stream per terminal.

use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::body::Incoming;
use hyper::{Method, Request};
use tracing::warn;
use trogon_sandbox_actor_agent::{TERMINAL_OP_PATH, TERMINAL_OPEN_PATH, TerminalFrame};
use trogon_sandbox_contract::{TerminalRequest, TerminalResponse};

use super::constants::HOST;
use super::{AgentError, GuestAgent, trim_ascii_whitespace};

/// The live frame stream of one guest terminal. Dropping it closes the vsock stream, which
/// makes the guest hang the terminal up, so a worker that forgets a terminal never leaves a
/// shell running.
pub struct TerminalStream {
    body: Incoming,
    buffer: Vec<u8>,
    target: String,
    driver: tokio::task::JoinHandle<()>,
}

impl TerminalStream {
    /// The next frame, or `None` when the guest closed the stream. Unreadable lines are
    /// skipped; a terminal that loses one frame is better than one that dies.
    pub async fn next_frame(&mut self) -> Result<Option<TerminalFrame>, AgentError> {
        loop {
            while let Some(end) = self.buffer.iter().position(|b| *b == b'\n') {
                let line: Vec<u8> = self.buffer.drain(..=end).collect();
                let line = trim_ascii_whitespace(&line);
                if line.is_empty() {
                    continue;
                }
                match serde_json::from_slice(line) {
                    Ok(frame) => return Ok(Some(frame)),
                    Err(e) => {
                        warn!(target = %self.target, error = %e, "dropping unreadable terminal frame")
                    }
                }
            }
            match self.body.frame().await {
                None => return Ok(None),
                Some(Err(e)) => {
                    return Err(AgentError::Unreachable {
                        target: self.target.clone(),
                        message: e.to_string(),
                    });
                }
                Some(Ok(frame)) => {
                    if let Ok(data) = frame.into_data() {
                        self.buffer.extend_from_slice(&data);
                    }
                }
            }
        }
    }
}

impl Drop for TerminalStream {
    fn drop(&mut self) {
        self.driver.abort();
    }
}

impl GuestAgent {
    /// Opens a terminal in the guest. The first frame the guest sends decides the outcome: a
    /// terminal that started comes back as `Opened` together with its live frame stream;
    /// anything else is the guest's verdict and carries no stream. A guest rootfs that
    /// predates terminals answers 404, reported as `Unsupported`.
    pub async fn terminal_open(
        &self,
        request: &TerminalRequest,
    ) -> Result<(TerminalResponse, Option<TerminalStream>), AgentError> {
        let opened = self
            .within(self.timeouts.control, async {
                let body = serde_json::to_vec(request).map_err(|e| self.unreachable(e.to_string()))?;
                let request = Request::builder()
                    .method(Method::POST)
                    .uri(TERMINAL_OPEN_PATH)
                    .header(hyper::header::HOST, HOST)
                    .header(hyper::header::CONTENT_TYPE, "application/json")
                    .body(Full::new(Bytes::from(body)))
                    .map_err(|e| self.unreachable(e.to_string()))?;
                let (response, driver) = self.send_tracked(request).await?;
                if response.status() == hyper::StatusCode::NOT_FOUND {
                    driver.abort();
                    return Ok(None);
                }
                let response = match self.check_status(response).await {
                    Ok(response) => response,
                    Err(e) => {
                        driver.abort();
                        return Err(e);
                    }
                };
                let mut stream = TerminalStream {
                    body: response.into_body(),
                    buffer: Vec::new(),
                    target: self.endpoint.to_string(),
                    driver,
                };
                match stream.next_frame().await? {
                    Some(TerminalFrame::Response(response)) => Ok(Some((response, stream))),
                    _ => Err(AgentError::Malformed {
                        target: self.endpoint.to_string(),
                        message: "terminal stream did not open with a response frame".to_string(),
                    }),
                }
            })
            .await?;
        Ok(match opened {
            None => (TerminalResponse::Unsupported, None),
            Some((response @ TerminalResponse::Opened { .. }, stream)) => (response, Some(stream)),
            Some((response, _)) => (response, None),
        })
    }

    /// Input, resize or close for a terminal the guest holds.
    pub async fn terminal_op(&self, request: &TerminalRequest) -> Result<TerminalResponse, AgentError> {
        self.call(TERMINAL_OP_PATH, request).await
    }
}
