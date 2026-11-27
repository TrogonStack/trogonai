use serde::{Deserialize, Serialize};

use crate::{LeaseError, PortAccess};

/// The reply to a [`GetSandboxDomainRequest`](crate::GetSandboxDomainRequest), tagged by `outcome`.
///
/// `Available.endpoint` is a `host:port` reachable from the caller. `Available.url` and `Available.expires_at` are
/// filled by the gateway when preview access control is enabled: `url` is an authenticated, tenant-bound address
/// to use instead of the bare endpoint, valid until `expires_at` (unix seconds). Workers and brokers never set
/// them. `Unsupported` means the provider cannot publish ports at all, as opposed to `PortNotExposed`, which means
/// the provider could, but this particular port was never declared in the sandbox spec's exposed ports.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "outcome")]
pub enum GetSandboxDomainResponse {
    Available {
        endpoint: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        url: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expires_at: Option<u64>,
        /// Set by workers whose published ports need a URL and headers rather than a bare endpoint.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        access: Option<PortAccess>,
    },
    PortNotExposed,
    Unsupported,
    /// The request carried `wait_ms` and the port still did not answer in that time. `waited_ms` is how long the
    /// gateway actually waited.
    NotReady {
        waited_ms: u64,
    },
    Denied(LeaseError),
}
