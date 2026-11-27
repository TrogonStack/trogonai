//! Dialing the vsock stream and sending one request over it.

use bytes::Bytes;
use http_body_util::Full;
use hyper::body::Incoming;
use hyper::{Request, Response};
use hyper_util::rt::TokioIo;
use tracing::warn;

use trogon_sandbox_actor_agent::{
    AGENT_PROTOCOL_VERSION_HEADER, GUEST_PROTOCOL_VERSION, WORKER_PROTOCOL_VERSION_HEADER, admit_agent_version,
};

use super::{AgentError, GuestAgent};

impl GuestAgent {
    /// Dials a fresh vsock stream and sends one request over it.
    ///
    /// Each call opens its own stream rather than sharing one: control traffic to one guest
    /// is low-volume, and a fresh stream never reuses a connection the guest already closed.
    pub(super) async fn send_once(&self, request: Request<Full<Bytes>>) -> Result<Response<Incoming>, AgentError> {
        self.send_tracked(request).await.map(|(response, _)| response)
    }

    /// [`Self::send_once`], also returning the task driving the connection so a long-lived
    /// stream can abort it (and so close the vsock stream) instead of waiting for the guest.
    pub(super) async fn send_tracked(
        &self,
        mut request: Request<Full<Bytes>>,
    ) -> Result<(Response<Incoming>, tokio::task::JoinHandle<()>), AgentError> {
        request.headers_mut().insert(
            WORKER_PROTOCOL_VERSION_HEADER,
            hyper::header::HeaderValue::from(GUEST_PROTOCOL_VERSION),
        );
        let stream = self
            .endpoint
            .connect()
            .await
            .map_err(|e| self.unreachable(e.to_string()))?;
        let (mut sender, connection) = hyper::client::conn::http1::handshake(TokioIo::new(stream))
            .await
            .map_err(|e| self.unreachable(format!("HTTP handshake over vsock: {e}")))?;
        let target = self.endpoint.to_string();
        let driver = tokio::spawn(async move {
            if let Err(e) = connection.await {
                warn!(target, error = %e, "guest agent vsock connection ended");
            }
        });
        match sender.send_request(request).await {
            Ok(response) => {
                // Refuse an agent that is missing the header or below the minimum before any
                // of its body is trusted.
                let header = response
                    .headers()
                    .get(AGENT_PROTOCOL_VERSION_HEADER)
                    .and_then(|v| v.to_str().ok());
                match admit_agent_version(header, self.min_agent_protocol_version) {
                    Ok(_) => Ok((response, driver)),
                    Err(reason) => {
                        driver.abort();
                        Err(AgentError::ProtocolMismatch {
                            target: self.endpoint.to_string(),
                            reason,
                        })
                    }
                }
            }
            Err(e) => {
                driver.abort();
                Err(self.unreachable(e.to_string()))
            }
        }
    }
}
