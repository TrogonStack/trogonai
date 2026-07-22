//! JSON request/response for the bridge ↔ callout **internal** mint subject
//! (`a2a.bridge.auth.callout.request`). Not used on `$SYS.REQ.USER.AUTH`.

use serde::{Deserialize, Serialize};

/// Internal bridge mint request (JSON). Mirrors the pre-wire-format illustrative shape.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BridgeMintRequest {
    pub user_nkey: Option<String>,
    pub user_jwt: Option<String>,
    pub account: Option<String>,
    pub client_info: Option<BridgeClientInfo>,
    pub connect_opts: Option<BridgeConnectOpts>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BridgeClientInfo {
    pub client_cert_pem: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BridgeConnectOpts {
    pub auth_scheme: Option<BridgeAuthScheme>,
    pub api_key: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BridgeAuthScheme {
    Oidc,
    MTls,
    ApiKey,
}

/// Internal bridge mint success response (JSON).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BridgeMintResponse {
    pub user_jwt: String,
}
