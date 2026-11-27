use serde::{Deserialize, Serialize};

/// Why a worker refused an attach, when it can say.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SshAttachRefusal {
    /// The provider has no way to serve SSH through the gateway.
    Unsupported,
}
