//! JSON request/response for the bridge ↔ callout **internal** mint subject
//! (`a2a.bridge.auth.callout.request`). Not used on `$SYS.REQ.USER.AUTH`.

use std::fmt;

use serde::{Deserialize, Serialize};

/// Internal bridge mint request (JSON). Mirrors the pre-wire-format illustrative shape.
#[derive(Clone, Serialize, Deserialize)]
pub struct BridgeMintRequest {
    pub user_nkey: Option<String>,
    pub user_jwt: Option<String>,
    pub account: Option<String>,
    pub client_info: Option<BridgeClientInfo>,
    pub connect_opts: Option<BridgeConnectOpts>,
}

impl fmt::Debug for BridgeMintRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BridgeMintRequest")
            .field("user_nkey", &self.user_nkey.as_ref().map(|_| "<redacted>"))
            .field("user_jwt", &self.user_jwt.as_ref().map(|_| "<redacted>"))
            .field("account", &self.account)
            .field("client_info", &self.client_info)
            .field("connect_opts", &self.connect_opts)
            .finish()
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BridgeClientInfo {
    pub client_cert_pem: Option<String>,
}

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct BridgeConnectOpts {
    pub auth_scheme: Option<BridgeAuthScheme>,
    pub api_key: Option<String>,
}

impl fmt::Debug for BridgeConnectOpts {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BridgeConnectOpts")
            .field("auth_scheme", &self.auth_scheme)
            .field("api_key", &self.api_key.as_ref().map(|_| "<redacted>"))
            .finish()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BridgeAuthScheme {
    Oidc,
    MTls,
    ApiKey,
}

/// Internal bridge mint success response (JSON).
#[derive(Clone, Serialize, Deserialize)]
pub struct BridgeMintResponse {
    pub user_jwt: String,
}

impl fmt::Debug for BridgeMintResponse {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BridgeMintResponse")
            .field("user_jwt", &"<redacted>")
            .finish()
    }
}

#[cfg(test)]
mod tests;
