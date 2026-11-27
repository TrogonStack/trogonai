use std::path::PathBuf;

use trogon_sandbox_contract::HostMountAccess;

/// A host directory mounted as the sandbox's workspace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostMount {
    pub path: PathBuf,
    pub access: HostMountAccess,
}
