use async_trait::async_trait;
use tokio::sync::mpsc;
use trogon_sandbox_contract::TenantId;

use crate::{
    SshAttachRequest, SshAttachResponse, SshDetachRequest, SshDetachResponse, SshSessionId, SshStreamFrame, TunnelError,
};

/// How the gateway reaches the worker that holds a lease.
#[async_trait]
pub trait SshTunnelBackend: Send + Sync {
    /// Asks the worker to accept `req` for `tenant`.
    async fn attach(&self, tenant: &TenantId, req: &SshAttachRequest) -> Result<SshAttachResponse, TunnelError>;

    /// Tells the worker the session of `req` ended.
    async fn detach(&self, tenant: &TenantId, req: &SshDetachRequest) -> Result<SshDetachResponse, TunnelError>;

    /// Sends one frame toward the worker.
    async fn publish_frame(&self, frame: &SshStreamFrame) -> Result<(), TunnelError>;

    /// Frames the worker sends for `session_id`. The receiver closes when the subscription ends.
    async fn subscribe_frames(&self, session_id: &SshSessionId) -> Result<mpsc::Receiver<SshStreamFrame>, TunnelError>;
}
