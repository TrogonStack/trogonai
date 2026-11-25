use serde::{Deserialize, Serialize};

use crate::LeaseError;

/// A worker's answer to a [`WaitSandboxReadyRequest`](crate::WaitSandboxReadyRequest), tagged by `outcome`.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "outcome")]
pub enum WaitSandboxReadyResponse {
    Ready,
    /// The timeout elapsed first. `pending_ports` are the ports last seen not accepting connections, at most
    /// [`ReadyPorts::MAX_LEN`](crate::ReadyPorts::MAX_LEN); empty while the lease itself was still pending.
    NotReady {
        #[serde(default)]
        pending_ports: Vec<u16>,
    },
    /// Ports were requested but the provider cannot run commands, or the sandbox has none of the tools the probe uses.
    Unsupported,
    Denied(LeaseError),
}
