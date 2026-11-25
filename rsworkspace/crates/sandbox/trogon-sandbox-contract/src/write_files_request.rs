use serde::{Deserialize, Serialize};

use crate::{LeaseId, SandboxFileWrite, SandboxUserName};

/// A request to write one or more files into an already-running lease's sandbox.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct WriteFilesRequest {
    pub lease_id: LeaseId,
    pub files: Vec<SandboxFileWrite>,
    /// When set, the write runs as this user: ownership and permissions follow that user, and relative paths in
    /// `files` resolve against the user's home directory instead of the lease's default working directory. `None`
    /// writes as the lease's configured user.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub as_user: Option<SandboxUserName>,
}
