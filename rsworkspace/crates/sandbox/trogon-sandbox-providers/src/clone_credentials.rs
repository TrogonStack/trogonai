use trogon_sandbox_contract::CloneAuth;

/// Where to clone a workspace from and how to authenticate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CloneCredentials {
    pub clone_url: String,
    pub auth: CloneAuth,
}
