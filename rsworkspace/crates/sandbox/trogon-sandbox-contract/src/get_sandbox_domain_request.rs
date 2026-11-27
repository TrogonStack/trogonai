use serde::{Deserialize, Serialize};

use crate::LeaseId;

/// Client-facing request to resolve a port the sandbox spec exposes on an already-running lease into a reachable
/// endpoint, which backs the client's `sandbox.domain(port)`.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct GetSandboxDomainRequest {
    pub lease_id: LeaseId,
    pub port: u16,
    /// Asks the gateway to hold the reply until the port answers an HTTP request, for at most this many
    /// milliseconds (clamped to [`GetSandboxDomainRequest::MAX_WAIT_MS`]). `None` or `0` resolves immediately.
    /// Brokers and workers ignore it; peers that predate it never send it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wait_ms: Option<u64>,
}

impl GetSandboxDomainRequest {
    /// Longest `wait_ms` a gateway honours on a domain request, in milliseconds.
    pub const MAX_WAIT_MS: u64 = 120_000;
}
