use nats_jwt_rs::authorization::AuthResponse;
use nkeys::{KeyPair, XKey};

use super::{NATS_JWT_PREFIX, ServerAuthRequestClaims};
use crate::error::AuthCalloutError;
use crate::jwt::MintedUserJwt;

/// Callout-signed authorization **response** JWT for `$SYS.REQ.USER.AUTH` reply.
#[derive(Debug, Clone)]
pub struct CalloutAuthResponseClaims {
    encoded: String,
}

impl CalloutAuthResponseClaims {
    pub fn success(
        request: &ServerAuthRequestClaims,
        user_jwt: &MintedUserJwt,
        callout_issuer: &KeyPair,
    ) -> Result<Self, AuthCalloutError> {
        let user_nkey = request.user_nkey()?;
        let mut claim = AuthResponse::generic_claim(user_nkey.as_str().to_owned());
        claim.nats.jwt = user_jwt.as_str().to_owned();
        claim.aud = Some(request.server_id().to_owned());
        let encoded = claim
            .encode(callout_issuer)
            .map_err(|e| AuthCalloutError::WireFormat(format!("encode authorization response: {e}")))?;
        Ok(Self { encoded })
    }

    pub fn denial(
        request: &ServerAuthRequestClaims,
        message: impl Into<String>,
        callout_issuer: &KeyPair,
    ) -> Result<Self, AuthCalloutError> {
        let user_nkey = request.user_nkey()?;
        let mut claim = AuthResponse::generic_claim(user_nkey.as_str().to_owned());
        claim.nats.error = message.into();
        claim.aud = Some(request.server_id().to_owned());
        let encoded = claim
            .encode(callout_issuer)
            .map_err(|e| AuthCalloutError::WireFormat(format!("encode authorization denial: {e}")))?;
        Ok(Self { encoded })
    }

    pub fn as_jwt_str(&self) -> &str {
        &self.encoded
    }

    pub fn into_wire_bytes(
        self,
        request: &ServerAuthRequestClaims,
        account_xkey: Option<&XKey>,
    ) -> Result<Vec<u8>, AuthCalloutError> {
        let jwt = self.encoded.into_bytes();
        let Some(server_xkey_pub) = request.server_one_time_xkey() else {
            return Ok(jwt);
        };
        let account_xkey = account_xkey.ok_or_else(|| {
            AuthCalloutError::WireFormat(
                "server requested encrypted response (server_id.xkey) but account XKey is not configured".into(),
            )
        })?;
        let recipient = XKey::from_public_key(server_xkey_pub)
            .map_err(|e| AuthCalloutError::WireFormat(format!("invalid server one-time XKey in request: {e}")))?;
        if jwt.starts_with(NATS_JWT_PREFIX) {
            account_xkey
                .seal(&jwt, &recipient)
                .map_err(|e| AuthCalloutError::WireFormat(format!("XKey encrypt response: {e}")))
        } else {
            Err(AuthCalloutError::WireFormat(
                "internal error: response JWT has unexpected encoding".into(),
            ))
        }
    }
}

#[cfg(test)]
mod tests;
