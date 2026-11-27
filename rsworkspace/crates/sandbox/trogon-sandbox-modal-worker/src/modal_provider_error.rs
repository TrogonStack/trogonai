use trogon_sandbox_contract::{ProviderKind, SandboxIdError, WorkerId};
use trogon_sandbox_protocol::{AuthorityResourceIdError, PrivilegedUnsupportedError};

use crate::{CpuCoresError, ModalApiError, WorkspaceError};

/// A lease or lease operation could not be carried out on Modal.
#[derive(Debug, thiserror::Error)]
pub enum ModalProviderError {
    #[error("invalid lease spec: {0}")]
    InvalidSpec(String),
    #[error("invalid network policy: {0}")]
    InvalidNetworkPolicy(String),
    #[error("snapshot unsupported on {provider:?}: {reason}")]
    SnapshotUnsupported { provider: ProviderKind, reason: String },
    #[error(transparent)]
    Privileged(#[from] PrivilegedUnsupportedError),
    #[error(transparent)]
    Api(#[from] ModalApiError),
    #[error(transparent)]
    Workspace(#[from] WorkspaceError),
    #[error("a sandbox id is unusable as a resource id: {0}")]
    ResourceId(#[from] AuthorityResourceIdError),
    #[error("a core request is unusable: {0}")]
    Cpu(#[from] CpuCoresError),
    #[error("a sandbox id is unusable: {0}")]
    SandboxId(#[from] SandboxIdError),
    #[error("the provider is already bound to worker {0}")]
    WorkerBound(WorkerId),
}
