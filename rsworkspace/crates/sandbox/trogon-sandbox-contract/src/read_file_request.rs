use serde::{Deserialize, Serialize};

use crate::{LeaseId, SandboxUserName};

/// A request to read a single file out of an already-running lease's sandbox.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct ReadFileRequest {
    pub lease_id: LeaseId,
    pub path: String,
    /// When set, the read runs as this user: permissions follow that user, and a relative `path` resolves against the
    /// user's home directory instead of the lease's default working directory. `None` reads as the lease's configured
    /// user.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub as_user: Option<SandboxUserName>,
}
