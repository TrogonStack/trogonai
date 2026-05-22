//! Authorization: SpiceDB-backed checks for gated MCP methods (`spicedb-rs-client`) or allow-all.

use std::fmt;

use async_trait::async_trait;
use serde::Serialize;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum IdentitySource {
    Jwt,
    LegacyHeader,
    Anonymous,
}

#[derive(Clone, Debug)]
pub struct GatewayIdentity {
    pub tenant: Option<String>,
    pub caller_sub: Option<String>,
    pub issuer: Option<String>,
    pub jti: Option<String>,
    pub source: IdentitySource,
}

#[derive(Clone, Debug)]
pub struct AuthzContext<'a> {
    pub tenant: Option<&'a str>,
    pub caller_sub: Option<&'a str>,
    pub identity_source: IdentitySource,
    pub server_id: &'a str,
    pub jsonrpc_method: &'a str,
    pub tool_name: Option<&'a str>,
    pub resource_uri: Option<&'a str>,
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
    async fn authorize_mcp_request(&self, ctx: AuthzContext<'_>) -> Result<bool, AuthzError>;
}

#[derive(Clone, Copy, Debug, Default)]
pub struct AllowAllPermissionChecker;

#[async_trait]
impl PermissionChecker for AllowAllPermissionChecker {
    async fn authorize_mcp_request(&self, _ctx: AuthzContext<'_>) -> Result<bool, AuthzError> {
        Ok(true)
    }
}
