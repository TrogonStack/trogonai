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

/// Internal bridge mint refusal (JSON). Carries only the opaque denial
/// category, so a refused caller learns no more than a refused server
/// connection does.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BridgeMintRefusal {
    pub refusal: String,
}

impl BridgeMintRefusal {
    pub fn new(category: impl Into<String>) -> Self {
        Self {
            refusal: category.into(),
        }
    }
}

/// What the callout answers on the bridge mint subject: a user JWT, or a
/// refusal and never both.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum BridgeMintReply {
    Minted(BridgeMintResponse),
    Refused(BridgeMintRefusal),
}

impl BridgeMintReply {
    /// The reply as the JSON the bridge decodes. Both variants hold only
    /// strings, so encoding cannot fail; the empty fallback would fail the
    /// bridge's decode at once instead of leaving it waiting out its timeout.
    pub fn to_payload(&self) -> Vec<u8> {
        serde_json::to_vec(self).unwrap_or_default()
    }
}

#[cfg(test)]
mod tests;
