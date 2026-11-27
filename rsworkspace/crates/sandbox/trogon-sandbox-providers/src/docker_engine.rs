use async_trait::async_trait;

use crate::{
    CheckpointCreateSpec, CheckpointRestoreSpec, ContainerRunSpec, ContainerStartResult, DockerError, InspectStatus,
};

#[async_trait]
pub trait DockerEngine: Send + Sync {
    async fn create_and_start(&self, spec: ContainerRunSpec) -> Result<ContainerStartResult, DockerError>;
    async fn inspect(&self, container_id: &str) -> Result<InspectStatus, DockerError>;
    async fn kill(&self, container_id: &str) -> Result<(), DockerError>;
    async fn commit(&self, container_id: &str, repo: &str, tag: &str) -> Result<String, DockerError>;
    async fn export_rootfs(&self, container_id: &str) -> Result<Vec<u8>, DockerError>;
    async fn import_rootfs(&self, tar: &[u8], repo: &str, tag: &str) -> Result<String, DockerError>;
    async fn create_checkpoint_leave_running(
        &self,
        container_id: &str,
        spec: &CheckpointCreateSpec,
    ) -> Result<(), DockerError>;
    async fn start_from_checkpoint(&self, container_id: &str, spec: &CheckpointRestoreSpec) -> Result<(), DockerError>;
    async fn create_container_stopped(&self, spec: ContainerRunSpec) -> Result<String, DockerError>;
    /// Ids of every container, running or not, whose label `key` equals `value`.
    async fn list_by_label(&self, key: &str, value: &str) -> Result<Vec<String>, DockerError>;
    /// Force-removes one container by id. A container that is already gone is not an error.
    async fn remove(&self, container_id: &str) -> Result<(), DockerError>;
}
