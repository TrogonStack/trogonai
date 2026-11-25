use std::fmt;

use serde::{Deserialize, Serialize};

/// Where and as whom to connect over SSH.
///
/// `user` may itself be the access token, so the whole value belongs to the lease's tenant only. `Debug` never
/// shows `user` or `command`.
#[derive(Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct SshAccess {
    pub host: String,
    pub port: u16,
    pub user: String,
    /// A ready-to-run `ssh` command line, when the provider supplies one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    /// When the access stops working, in unix seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<u64>,
}

impl fmt::Debug for SshAccess {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SshAccess")
            .field("host", &self.host)
            .field("port", &self.port)
            .field("user", &"[REDACTED]")
            .field("command", &self.command.as_ref().map(|_| "[REDACTED]"))
            .field("expires_at", &self.expires_at)
            .finish()
    }
}
