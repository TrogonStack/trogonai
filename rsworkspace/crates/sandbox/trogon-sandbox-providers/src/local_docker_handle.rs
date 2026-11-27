use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;

use crate::{DockerEngine, InspectStatus, SandboxError, SandboxHandle, SandboxStatus};

/// A container started by [`crate::LocalDockerProvider`].
pub struct LocalDockerHandle {
    pub(crate) id: String,
    pub(crate) sidecar_id: Option<String>,
    pub(crate) policy_file_path: Option<PathBuf>,
    pub(crate) engine: Arc<dyn DockerEngine>,
    pub(crate) port_endpoints: HashMap<u16, String>,
}

#[async_trait]
impl SandboxHandle for LocalDockerHandle {
    fn id(&self) -> &str {
        &self.id
    }

    fn policy_file_path(&self) -> Option<&Path> {
        self.policy_file_path.as_deref()
    }

    fn sidecar_id(&self) -> Option<&str> {
        self.sidecar_id.as_deref()
    }

    fn port_endpoints(&self) -> HashMap<u16, String> {
        self.port_endpoints.clone()
    }

    async fn status(&self) -> SandboxStatus {
        let status = match self.engine.inspect(&self.id).await {
            Ok(status) => status,
            Err(error) => {
                return SandboxStatus::Failed {
                    error: format!("failed to inspect container: {error}"),
                };
            }
        };

        match status {
            InspectStatus::Running => SandboxStatus::Running,
            InspectStatus::Pending => SandboxStatus::Pending,
            InspectStatus::Exited(0) => SandboxStatus::Completed,
            InspectStatus::Exited(code) => SandboxStatus::Failed {
                error: format!("container exited with code {code}"),
            },
            InspectStatus::Failed(error) => SandboxStatus::Failed { error },
        }
    }

    async fn cancel(&self) -> Result<(), SandboxError> {
        if let Some(sidecar_id) = &self.sidecar_id {
            self.engine.kill(sidecar_id).await?;
        }
        self.engine.kill(&self.id).await?;
        Ok(())
    }
}
