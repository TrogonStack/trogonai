use serde::{Deserialize, Serialize};

use crate::{ExecutionId, LeaseId};

/// A client request to end a detached command gracefully.
///
/// The worker sends SIGTERM, then SIGKILL if the command is still running once `grace_seconds` have passed. An
/// omitted `grace_seconds` reads as [`StopCommandRequest::DEFAULT_GRACE_SECONDS`].
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct StopCommandRequest {
    pub lease_id: LeaseId,
    pub execution_id: ExecutionId,
    #[serde(default = "default_grace_seconds")]
    pub grace_seconds: u32,
}

impl StopCommandRequest {
    /// The grace period applied when `grace_seconds` is unset.
    pub const DEFAULT_GRACE_SECONDS: u32 = 10;
    /// The longest grace period a caller may ask for. Larger requests are meant to be clamped to it.
    pub const MAX_GRACE_SECONDS: u32 = 300;
}

fn default_grace_seconds() -> u32 {
    StopCommandRequest::DEFAULT_GRACE_SECONDS
}
