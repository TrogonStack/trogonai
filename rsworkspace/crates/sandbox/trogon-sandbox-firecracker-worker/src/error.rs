use crate::agent::AgentError;
use crate::api::ApiError;
use crate::vm_dir::VmDirError;

#[derive(Debug, thiserror::Error)]
pub enum ProviderError {
    /// A field this provider cannot honor. The broker does not validate against this
    /// worker's capabilities, so this is the authoritative refusal.
    #[error("{0}")]
    InvalidSpec(String),
    #[error("worker is at capacity: {max} microVMs already leased")]
    AtCapacity { max: u32 },
    #[error(transparent)]
    VmDir(#[from] VmDirError),
    #[error("copying the base rootfs into the lease directory failed: {0}")]
    RootfsCopy(String),
    #[error("could not launch firecracker: {0}")]
    Launch(String),
    #[error("firecracker exited before its API socket came up: {0}")]
    VmmExited(String),
    #[error("firecracker API socket did not appear within {seconds}s")]
    ApiSocketTimeout { seconds: u64 },
    #[error(transparent)]
    Api(#[from] ApiError),
    #[error(transparent)]
    Agent(#[from] AgentError),
    #[error("could not create the working directory {dir} in the guest: {reason}")]
    WorkingDir { dir: String, reason: String },
    #[error("lease network: {0}")]
    Network(String),
    #[error(
        "snapshot {0} is not on this worker's host: snapshots stay where they were taken, so restore on the worker that took it"
    )]
    SnapshotNotHere(String),
    #[error("the restored guest could not take its new lease's identity: {0}")]
    Identity(String),
}
