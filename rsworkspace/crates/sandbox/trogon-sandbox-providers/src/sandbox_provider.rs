use crate::{SandboxError, SandboxHandle, SandboxSpec};

/// Launches sandboxes on one engine.
#[async_trait::async_trait]
pub trait SandboxProvider: Send + Sync {
    async fn launch(&self, spec: SandboxSpec) -> Result<Box<dyn SandboxHandle>, SandboxError>;
}
