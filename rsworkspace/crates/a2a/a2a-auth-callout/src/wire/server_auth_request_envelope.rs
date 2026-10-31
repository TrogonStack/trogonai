use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use nats_jwt_rs::Claims;
use nats_jwt_rs::authorization::AuthRequest;
use nkeys::{KeyPair, KeyPairType};

use super::{
    AUTH_REQUEST_AUDIENCE, AUTH_REQUEST_XKEY_HEADER, AuthRequestError, NATS_JWT_PREFIX, ServerAuthRequestClaims,
    XkeyPublic,
};
use crate::admission_keys::{AuthAccount, CalloutXkey};

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

    /// Accepts a request from any server of the `AUTH` account. Every
    /// nats-server generates its signing key and xkey at boot, so neither can
    /// be pinned; the request must instead be signed by a server key, name
    /// that key as its server id, and be issued for the `AUTH` account.
    pub fn decode(
        self,
        auth_account: &AuthAccount,
        callout_xkey: Option<&CalloutXkey>,
        server_xkey: Option<&XkeyPublic>,
    ) -> Result<ServerAuthRequestClaims, AuthRequestError> {
        let (token, sealed_by) = self.open(callout_xkey, server_xkey)?;
        let token = std::str::from_utf8(&token).map_err(|_| AuthRequestError::NotAJwt)?;
        let (claims, nats_json) = verify_server_signature(token)?;
        if claims.aud.as_deref() != Some(AUTH_REQUEST_AUDIENCE) {
            return Err(AuthRequestError::WrongAudience);
        }
        if claims.sub != auth_account.as_str() {
            return Err(AuthRequestError::WrongAccount);
        }
        if claims.nats.server.id != claims.iss {
            return Err(AuthRequestError::ServerIdMismatch);
        }
        let named_xkey = claims.nats.server.xkey.as_deref().filter(|xkey| !xkey.is_empty());
        match (sealed_by, named_xkey) {
            (None, None) => {}
            (None, Some(_)) => return Err(AuthRequestError::PlaintextAsksForEncryption),
            (Some(sealer), Some(named)) if sealer.as_str() == named => {}
            (Some(_), _) => return Err(AuthRequestError::ServerXkeyMismatch),
        }
        Ok(ServerAuthRequestClaims::from_decoded(claims, nats_json))
    }

    pub fn decode_from_message(
        payload: Vec<u8>,
        headers: Option<&async_nats::HeaderMap>,
        auth_account: &AuthAccount,
        callout_xkey: Option<&CalloutXkey>,
    ) -> Result<ServerAuthRequestClaims, AuthRequestError> {
        let server_xkey = headers
            .and_then(|headers| headers.get(AUTH_REQUEST_XKEY_HEADER))
            .map(|value| XkeyPublic::parse(value.as_str()).map_err(|_| AuthRequestError::InvalidServerXkey))
            .transpose()?;
        Self::from_bytes(payload).decode(auth_account, callout_xkey, server_xkey.as_ref())
    }

    fn open<'a>(
        self,
        callout_xkey: Option<&CalloutXkey>,
        server_xkey: Option<&'a XkeyPublic>,
    ) -> Result<(Vec<u8>, Option<&'a XkeyPublic>), AuthRequestError> {
        let plaintext = self.0.starts_with(NATS_JWT_PREFIX);
        match (callout_xkey, plaintext) {
            (None, true) => Ok((self.0, None)),
            (None, false) => Err(AuthRequestError::UnexpectedEncryption),
            (Some(_), true) => Err(AuthRequestError::UnexpectedPlaintext),
            (Some(callout_xkey), false) => {
                let server_xkey = server_xkey.ok_or(AuthRequestError::MissingServerXkey)?;
                let token = callout_xkey
                    .open(&self.0, server_xkey)
                    .map_err(AuthRequestError::Unsealable)?;
                Ok((token, Some(server_xkey)))
            }
        }
    }
}

fn verify_server_signature(token: &str) -> Result<(Claims<AuthRequest>, serde_json::Value), AuthRequestError> {
    let mut segments = token.split('.');
    let (Some(header), Some(payload_segment), Some(signature), None) =
        (segments.next(), segments.next(), segments.next(), segments.next())
    else {
        return Err(AuthRequestError::NotAJwt);
    };
    let signature = URL_SAFE_NO_PAD
        .decode(signature)
        .map_err(|_| AuthRequestError::NotAJwt)?;
    let payload_bytes = URL_SAFE_NO_PAD
        .decode(payload_segment)
        .map_err(|_| AuthRequestError::NotAJwt)?;
    let mut payload: serde_json::Value = serde_json::from_slice(&payload_bytes).map_err(AuthRequestError::Claims)?;

    let server = payload
        .get("iss")
        .and_then(serde_json::Value::as_str)
        .and_then(|issuer| KeyPair::from_public_key(issuer).ok())
        .filter(|issuer| issuer.key_pair_type() == KeyPairType::Server)
        .ok_or(AuthRequestError::IssuerNotAServer)?;
    let signed = &token[..header.len() + 1 + payload_segment.len()];
    server
        .verify(signed.as_bytes(), &signature)
        .map_err(|_| AuthRequestError::BadSignature)?;

    let nats_json = payload.get("nats").cloned().unwrap_or_default();
    normalize_auth_request_payload(&mut payload);
    let claims = serde_json::from_value(payload).map_err(AuthRequestError::Claims)?;
    Ok((claims, nats_json))
}

fn normalize_auth_request_payload(payload: &mut serde_json::Value) {
    let Some(client_info) = payload
        .get_mut("nats")
        .and_then(|nats| nats.get_mut("client_info"))
        .and_then(|ci| ci.as_object_mut())
    else {
        return;
    };
    client_info
        .entry("name_tag")
        .or_insert_with(|| serde_json::Value::String(String::new()));
    client_info
        .entry("nonce")
        .or_insert_with(|| serde_json::Value::String(String::new()));

    let Some(tls) = payload
        .get_mut("nats")
        .and_then(|nats| nats.get_mut("client_tls"))
        .and_then(|tls| tls.as_object_mut())
    else {
        return;
    };
    tls.entry("certs")
        .or_insert_with(|| serde_json::Value::Array(Vec::new()));
    tls.entry("verified_chains")
        .or_insert_with(|| serde_json::Value::Array(Vec::new()));
}

#[cfg(test)]
mod tests;
