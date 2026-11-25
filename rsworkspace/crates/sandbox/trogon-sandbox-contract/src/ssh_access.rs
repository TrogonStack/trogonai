use serde::{Deserialize, Serialize};

/// Where and as whom to connect over SSH.
///
/// `user` may itself be the access token, so the whole value belongs to the lease's tenant only.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
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
