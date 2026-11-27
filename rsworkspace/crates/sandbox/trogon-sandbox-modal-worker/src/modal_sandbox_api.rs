use std::time::Duration;

use async_trait::async_trait;
use trogon_sandbox_contract::{NativeSnapshotId, SandboxId};

use crate::{ExecResult, ModalApiError, ModalSandbox, ModalSandboxSpec, ModalTunnel};

/// The only thing the provider depends on to drive Modal, so lease and cancel logic stay testable without a
/// Modal workspace and the transport can change without touching the provider.
#[async_trait]
pub trait ModalSandboxApi: Send + Sync {
    async fn create_sandbox(&self, spec: &ModalSandboxSpec) -> Result<ModalSandbox, ModalApiError>;

    async fn get_sandbox(&self, sandbox_id: &SandboxId) -> Result<ModalSandbox, ModalApiError>;

    /// Polls until the sandbox runs. A sandbox Modal does not know yet counts as not ready.
    async fn wait_running(
        &self,
        sandbox_id: &SandboxId,
        max_attempts: u32,
        poll_interval: Duration,
    ) -> Result<ModalSandbox, ModalApiError> {
        for attempt in 1..=max_attempts {
            match self.get_sandbox(sandbox_id).await {
                Ok(sandbox) if sandbox.is_running() => return Ok(sandbox),
                Ok(_) | Err(ModalApiError::SandboxNotFound(_)) => {}
                Err(error) => return Err(error),
            }
            if attempt < max_attempts {
                tokio::time::sleep(poll_interval).await;
            }
        }
        Err(ModalApiError::SandboxNotReady { attempts: max_attempts })
    }

    async fn exec(&self, sandbox_id: &SandboxId, command: &[String]) -> Result<ExecResult, ModalApiError>;

    async fn write_file(&self, sandbox_id: &SandboxId, remote_path: &str, data: &[u8]) -> Result<(), ModalApiError>;

    async fn terminate(&self, sandbox_id: &SandboxId) -> Result<(), ModalApiError>;

    /// Tunnels for the ports the sandbox was created with.
    async fn tunnels(&self, sandbox_id: &SandboxId) -> Result<Vec<ModalTunnel>, ModalApiError>;

    /// Snapshots the sandbox filesystem and returns the id of the resulting `modal.Image`, which Modal retains
    /// for `ttl_seconds`.
    async fn snapshot_filesystem(
        &self,
        sandbox_id: &SandboxId,
        ttl_seconds: u64,
    ) -> Result<NativeSnapshotId, ModalApiError>;
}
