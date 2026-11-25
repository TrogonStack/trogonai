use serde::{Deserialize, Serialize};

use crate::{ExecutionId, LeaseId};

/// A request to wait until a process's readiness probe passes, it exits, or `timeout_seconds` elapse.
///
/// An omitted `timeout_seconds` reads as [`WaitReadyRequest::DEFAULT_TIMEOUT_SECONDS`].
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct WaitReadyRequest {
    pub lease_id: LeaseId,
    pub execution_id: ExecutionId,
    #[serde(default = "default_timeout_seconds")]
    pub timeout_seconds: u32,
}

impl WaitReadyRequest {
    /// The wait applied when `timeout_seconds` is omitted, in seconds.
    pub const DEFAULT_TIMEOUT_SECONDS: u32 = 30;
    /// The longest a single wait may block, in seconds. Larger requests are meant to be clamped to it.
    pub const MAX_TIMEOUT_SECONDS: u32 = 300;
}

fn default_timeout_seconds() -> u32 {
    WaitReadyRequest::DEFAULT_TIMEOUT_SECONDS
}
