use std::sync::Arc;
use std::time::Duration;

use async_nats::{HeaderMap, Request};
use async_trait::async_trait;
use bytes::Bytes;
use futures::StreamExt;
use serde::Serialize;
use serde::de::DeserializeOwned;
use tokio::sync::mpsc;
use tracing::warn;
use trogon_sandbox_contract::{LeaseId, TenantId};
use trogon_sandbox_protocol::{AUTH_HEADER, DispatchSigner, VerbScope};

use crate::constants::{DEFAULT_REQUEST_TIMEOUT, FRAME_BUFFER};
use crate::subjects::{ssh_attach, ssh_detach, ssh_stream_from_worker, ssh_stream_to_worker};
use crate::{
    SshAttachRefusal, SshAttachRequest, SshAttachResponse, SshDetachRequest, SshDetachResponse, SshSessionId,
    SshStreamFrame, SshTunnelBackend, TunnelError,
};

/// The tunnel over core NATS: request/reply for attach and detach, pub/sub for frames.
pub struct NatsSshTunnelBackend {
    client: async_nats::Client,
    request_timeout: Duration,
    signer: Option<Arc<DispatchSigner>>,
}

impl NatsSshTunnelBackend {
    pub fn new(client: async_nats::Client) -> Self {
        Self {
            client,
            request_timeout: DEFAULT_REQUEST_TIMEOUT,
            signer: None,
        }
    }

    /// Signs attach and detach requests, so workers can bind them to a tenant.
    pub fn with_signer(mut self, signer: Arc<DispatchSigner>) -> Self {
        self.signer = Some(signer);
        self
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.request_timeout = timeout;
        self
    }

    async fn call<R: DeserializeOwned>(
        &self,
        subject: String,
        lease: &LeaseId,
        tenant: &TenantId,
        body: &impl Serialize,
    ) -> Result<R, TunnelError> {
        let payload = serde_json::to_vec(body).map_err(TunnelError::Encode)?;
        let mut request = Request::new();
        if let Some(signer) = &self.signer {
            let scope = VerbScope {
                tenant: tenant.clone(),
                lease_id: lease.clone(),
                worker_id: None,
                attempt_id: None,
                epoch: None,
            };
            let mut headers = HeaderMap::new();
            headers.insert(AUTH_HEADER, signer.sign_verb(&subject, &payload, &scope)?.as_str());
            request = request.headers(headers);
        }
        let request = request.payload(Bytes::from(payload));
        let reply = tokio::time::timeout(self.request_timeout, self.client.send_request(subject, request))
            .await
            .map_err(|_| TunnelError::Timeout(self.request_timeout))??;
        serde_json::from_slice(&reply.payload).map_err(TunnelError::Decode)
    }
}

/// Turns a worker's attach reply into the tunnel's result, keeping a typed refusal apart from a failure the worker
/// did not classify.
fn attach_outcome(resp: SshAttachResponse) -> Result<SshAttachResponse, TunnelError> {
    if resp.ok {
        return Ok(resp);
    }
    let message = resp.message.unwrap_or_else(|| "attach denied".to_string());
    Err(match resp.refusal {
        Some(SshAttachRefusal::Unsupported) => TunnelError::Unsupported(message),
        None => TunnelError::AttachRejected(message),
    })
}

fn detach_outcome(resp: SshDetachResponse) -> Result<SshDetachResponse, TunnelError> {
    if resp.ok {
        return Ok(resp);
    }
    Err(TunnelError::DetachRejected(
        resp.message.unwrap_or_else(|| "detach denied".to_string()),
    ))
}

#[async_trait]
impl SshTunnelBackend for NatsSshTunnelBackend {
    async fn attach(&self, tenant: &TenantId, req: &SshAttachRequest) -> Result<SshAttachResponse, TunnelError> {
        let resp = self.call(ssh_attach(&req.lease_id), &req.lease_id, tenant, req).await?;
        attach_outcome(resp)
    }

    async fn detach(&self, tenant: &TenantId, req: &SshDetachRequest) -> Result<SshDetachResponse, TunnelError> {
        let resp = self.call(ssh_detach(&req.lease_id), &req.lease_id, tenant, req).await?;
        detach_outcome(resp)
    }

    async fn publish_frame(&self, frame: &SshStreamFrame) -> Result<(), TunnelError> {
        let payload = serde_json::to_vec(frame).map_err(TunnelError::Encode)?;
        self.client
            .publish(ssh_stream_to_worker(&frame.session_id), Bytes::from(payload))
            .await?;
        Ok(())
    }

    async fn subscribe_frames(&self, session_id: &SshSessionId) -> Result<mpsc::Receiver<SshStreamFrame>, TunnelError> {
        let mut subscription = self.client.subscribe(ssh_stream_from_worker(session_id)).await?;
        let (tx, rx) = mpsc::channel(FRAME_BUFFER);
        tokio::spawn(async move {
            loop {
                let message = tokio::select! {
                    _ = tx.closed() => break,
                    message = subscription.next() => message,
                };
                let Some(message) = message else { break };
                match serde_json::from_slice::<SshStreamFrame>(&message.payload) {
                    Ok(frame) => {
                        if tx.send(frame).await.is_err() {
                            break;
                        }
                    }
                    Err(error) => warn!(%error, "invalid ssh stream frame"),
                }
            }
        });
        Ok(rx)
    }
}
