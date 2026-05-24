use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;

use super::{NkeyPublic, NkeySeed, ServerAuthRequestClaims, XkeyPublic, NATS_JWT_PREFIX};
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
                AuthCalloutError::WireFormat(
                    "encrypted authorization request requires AUTH_CALLOUT_XKEY_SEED".into(),
                )
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

        let token = std::str::from_utf8(&jwt_bytes).map_err(|e| {
            AuthCalloutError::WireFormat(format!("authorization request JWT is not UTF-8: {e}"))
        })?;

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
    let root: serde_json::Value = serde_json::from_slice(&decoded)
        .map_err(|e| AuthCalloutError::WireFormat(format!("JWT payload JSON: {e}")))?;
    root.get("nats")
        .cloned()
        .ok_or_else(|| AuthCalloutError::WireFormat("JWT payload missing nats section".into()))
}

impl ServerAuthRequestEnvelope {
    pub fn decode_from_message(
        payload: Vec<u8>,
        _headers: Option<&async_nats::HeaderMap>,
        server_issuer: &NkeyPublic,
        account_xkey_seed: Option<&NkeySeed>,
        server_xkey_public: Option<&XkeyPublic>,
    ) -> Result<ServerAuthRequestClaims, AuthCalloutError> {
        let _ = _headers;
        Self::from_bytes(payload).decode(
            server_issuer,
            account_xkey_seed,
            server_xkey_public,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wire::test_encode::signed_auth_request;
    use nkeys::{KeyPair, XKey};

    #[test]
    fn roundtrip_plain_jwt_request() {
        let server = KeyPair::new_account();
        let server_pub = NkeyPublic::parse(server.public_key()).unwrap();
        let user = KeyPair::new_user();
        let token = signed_auth_request(&server, &user, |_| {});
        let decoded = ServerAuthRequestEnvelope::from_bytes(token.into_bytes())
            .decode(&server_pub, None, None)
            .unwrap();
        assert_eq!(decoded.user_nkey().unwrap().as_str(), user.public_key());
    }

    #[test]
    fn xkey_encrypted_request_roundtrip() {
        let server = KeyPair::new_account();
        let server_pub = NkeyPublic::parse(server.public_key()).unwrap();
        let account_xkey = XKey::new();
        let account_seed = NkeySeed::parse(account_xkey.seed().unwrap()).unwrap();

        let server_xkey = XKey::new();
        let server_xkey_pub = XkeyPublic::parse(server_xkey.public_key()).unwrap();
        let user = KeyPair::new_user();
        let token = signed_auth_request(&server, &user, |_| {});
        let sealed = server_xkey.seal(token.as_bytes(), &account_xkey).unwrap();

        let decoded = ServerAuthRequestEnvelope::from_bytes(sealed)
            .decode(&server_pub, Some(&account_seed), Some(&server_xkey_pub))
            .unwrap();
        assert_eq!(decoded.user_nkey().unwrap().as_str(), user.public_key());
    }

    #[test]
    fn rejects_tampered_signature() {
        let server = KeyPair::new_account();
        let server_pub = NkeyPublic::parse(server.public_key()).unwrap();
        let user = KeyPair::new_user();
        let mut token = signed_auth_request(&server, &user, |_| {});
        let last = token.pop().unwrap();
        token.push(if last == 'a' { 'b' } else { 'a' });
        let err = ServerAuthRequestEnvelope::from_bytes(token.into_bytes())
            .decode(&server_pub, None, None)
            .unwrap_err();
        assert!(
            err.to_string().contains("decode")
                || err.to_string().contains("verify")
                || err.to_string().contains("signature")
        );
    }
}
