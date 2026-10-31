//! JSON request and response of the internal mint exchange between the bridge
//! and the callout (`a2a.bridge.auth.callout.request`). Not used on
//! `$SYS.REQ.USER.AUTH`.

mod sealed_credential;

use std::fmt;

use serde::{Deserialize, Serialize};

pub use sealed_credential::{BridgeCredential, BridgeCredentialError, SealedBridgeCredential};

/// Internal bridge mint request (JSON). The caller's external credential only
/// travels sealed to the callout xkey, so a document that names a plaintext
/// credential field does not decode.
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BridgeMintRequest {
    pub user_nkey: Option<String>,
    pub account: Option<String>,
    pub client_info: Option<BridgeClientInfo>,
    pub sealed_credential: Option<SealedBridgeCredential>,
}

impl fmt::Debug for BridgeMintRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BridgeMintRequest")
            .field("user_nkey", &self.user_nkey.as_ref().map(|_| "<redacted>"))
            .field("account", &self.account)
            .field("client_info", &self.client_info)
            .field("sealed_credential", &self.sealed_credential)
            .finish()
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BridgeClientInfo {
    pub client_cert_pem: Option<String>,
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
