use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;

use super::{NATS_JWT_PREFIX, NkeyPublic, NkeySeed, ServerAuthRequestClaims, XkeyPublic};
use crate::error::AuthCalloutError;

/// Raw bytes published by `nats-server` on `$SYS.REQ.USER.AUTH` (JWT or XKey-encrypted JWT).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerAuthRequestEnvelope(Vec<u8>);

impl ServerAuthRequestEnvelope {
    pub fn from_bytes(bytes: Vec<u8>) -> Self {
        Self(bytes)
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    /// Decode and verify the server-signed authorization request.
    pub fn decode(
        self,
        server_issuer: &NkeyPublic,
        account_xkey_seed: Option<&NkeySeed>,
        server_xkey_public: Option<&XkeyPublic>,
    ) -> Result<ServerAuthRequestClaims, AuthCalloutError> {
        let jwt_bytes = if self.0.starts_with(NATS_JWT_PREFIX) {
            if account_xkey_seed.is_some() {
                return Err(AuthCalloutError::WireFormat(
                    "received unencrypted authorization request but account XKey is configured; \
                     server must set authorization.auth_callout.xkey"
                        .into(),
                ));
            }
            self.0
        } else {
            let seed = account_xkey_seed.ok_or_else(|| {
                AuthCalloutError::WireFormat("encrypted authorization request requires AUTH_CALLOUT_XKEY_SEED".into())
            })?;
            let server_xkey_pub = server_xkey_public.ok_or_else(|| {
                AuthCalloutError::WireFormat(
                    "encrypted authorization request requires AUTH_CALLOUT_SERVER_XKEY_PUBLIC".into(),
                )
            })?;
            let account_xkey = seed.to_xkey()?;
            let server_xkey = server_xkey_pub.to_xkey()?;
            account_xkey
                .open(&self.0, &server_xkey)
                .map_err(|e| AuthCalloutError::WireFormat(format!("XKey decrypt request: {e}")))?
        };

        let token = std::str::from_utf8(&jwt_bytes)
            .map_err(|e| AuthCalloutError::WireFormat(format!("authorization request JWT is not UTF-8: {e}")))?;

        let claims = server_issuer.verify_jwt_issuer(token)?;
        let nats_json = jwt_payload_nats_section(token)?;
        Ok(ServerAuthRequestClaims::from_decoded(claims, nats_json))
    }
}

fn jwt_payload_nats_section(token: &str) -> Result<serde_json::Value, AuthCalloutError> {
    let parts: Vec<&str> = token.split('.').collect();
    if parts.len() != 3 {
        return Err(AuthCalloutError::WireFormat("invalid JWT segment count".into()));
    }
    let decoded = URL_SAFE_NO_PAD
        .decode(parts[1].as_bytes())
        .map_err(|e| AuthCalloutError::WireFormat(format!("JWT payload base64: {e}")))?;
    let root: serde_json::Value =
        serde_json::from_slice(&decoded).map_err(|e| AuthCalloutError::WireFormat(format!("JWT payload JSON: {e}")))?;
    root.get("nats")
        .cloned()
        .ok_or_else(|| AuthCalloutError::WireFormat("JWT payload missing nats section".into()))
}

impl ServerAuthRequestEnvelope {
    pub fn decode_from_message(
        payload: Vec<u8>,
        headers: Option<&async_nats::HeaderMap>,
        server_issuer: &NkeyPublic,
        account_xkey_seed: Option<&NkeySeed>,
        server_xkey_public: Option<&XkeyPublic>,
    ) -> Result<ServerAuthRequestClaims, AuthCalloutError> {
        // Encrypted callout requests carry the per-request server XKey on the
        // `Nats-Server-Xkey` NATS header; the static `server_xkey_public`
        // arg is the legacy/fallback config knob. Header wins so live
        // nats-server traffic decrypts correctly.
        let header_xkey = headers
            .and_then(|h| h.get(super::AUTH_REQUEST_XKEY_HEADER))
            .map(|v| XkeyPublic::parse(v.as_str()))
            .transpose()?;
        let effective_server_xkey = header_xkey.as_ref().or(server_xkey_public);
        Self::from_bytes(payload).decode(server_issuer, account_xkey_seed, effective_server_xkey)
    }
}

#[cfg(test)]
mod tests;
