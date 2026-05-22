//! Phase-1 authorization: async callback for Zanzibar checks (SpiceDB later).

use std::fmt;

use async_trait::async_trait;

#[derive(Clone, Debug)]
pub struct AuthzContext<'a> {
    pub tenant: Option<&'a str>,
    pub server_id: &'a str,
    pub jsonrpc_method: &'a str,
    pub tool_name: Option<&'a str>,
}

#[derive(Debug)]
pub struct AuthzError(pub String);

impl fmt::Display for AuthzError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for AuthzError {}

#[async_trait]
pub trait PermissionChecker: Send + Sync {
    async fn check_tool_call(&self, ctx: AuthzContext<'_>) -> Result<bool, AuthzError>;
}

#[derive(Clone, Copy, Debug, Default)]
pub struct AllowAllPermissionChecker;

#[async_trait]
impl PermissionChecker for AllowAllPermissionChecker {
    async fn check_tool_call(&self, _ctx: AuthzContext<'_>) -> Result<bool, AuthzError> {
        Ok(true)
    }
}
