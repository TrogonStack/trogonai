use trogon_sandbox_oci::OciError;

use crate::KubeVirtOpsError;

/// A snapshot could not be exported to the registry.
#[derive(Debug, thiserror::Error)]
pub enum ExportError {
    #[error("the cluster operation failed: {0}")]
    Ops(#[from] KubeVirtOpsError),
    #[error("the registry operation failed: {0}")]
    Registry(#[from] OciError),
    #[error("an online snapshot needs VM {vm_name} to be running")]
    NotRunning { vm_name: String },
}
