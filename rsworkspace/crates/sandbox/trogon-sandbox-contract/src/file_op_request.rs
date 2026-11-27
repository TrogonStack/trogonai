use serde::{Deserialize, Serialize};

use crate::{FileOp, LeaseId, SandboxUserName};

/// A request for one structured file operation inside a running lease's sandbox.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct FileOpRequest {
    pub lease_id: LeaseId,
    pub op: FileOp,
    /// When set, the operation runs as this user. A worker that cannot honor it answers with
    /// an unsupported reply instead of running the operation as another identity.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub as_user: Option<SandboxUserName>,
}
