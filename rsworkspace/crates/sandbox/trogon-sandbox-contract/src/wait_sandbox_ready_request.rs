use serde::{Deserialize, Serialize};

use crate::{LeaseId, ReadyPorts};

/// A request to wait until a sandbox is usable.
///
/// That means the lease is running, which the broker only records after the worker materialized the workspace, and
/// every port in `ports` accepts a TCP connection on the sandbox loopback. An omitted `timeout_seconds` reads as
/// [`WaitSandboxReadyRequest::DEFAULT_TIMEOUT_SECONDS`].
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct WaitSandboxReadyRequest {
    pub lease_id: LeaseId,
    /// Sandbox-side ports that must accept connections. Empty checks the lease state only.
    #[serde(default)]
    pub ports: ReadyPorts,
    #[serde(default = "default_timeout_seconds")]
    pub timeout_seconds: u32,
}

impl WaitSandboxReadyRequest {
    /// The wait applied when `timeout_seconds` is omitted, in seconds.
    pub const DEFAULT_TIMEOUT_SECONDS: u32 = 30;
    /// The longest a single wait may block, in seconds. Larger requests are meant to be clamped to it.
    pub const MAX_TIMEOUT_SECONDS: u32 = 300;
}

fn default_timeout_seconds() -> u32 {
    WaitSandboxReadyRequest::DEFAULT_TIMEOUT_SECONDS
}
