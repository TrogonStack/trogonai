use async_trait::async_trait;

use crate::{GuestExec, GuestProbe};

/// A [`GuestProbe`] that runs `echo` in the guest through [`GuestExec`].
#[derive(Debug, Clone, Default)]
pub struct ExecGuestProbe {
    exec: GuestExec,
}

impl ExecGuestProbe {
    /// Probes through `exec`.
    pub fn new(exec: GuestExec) -> Self {
        Self { exec }
    }
}

#[async_trait]
impl GuestProbe for ExecGuestProbe {
    async fn ready(&self, namespace: &str, vm_name: &str) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        self.exec.run(namespace, vm_name, "echo sandbox-ready").await?;
        Ok(())
    }
}
