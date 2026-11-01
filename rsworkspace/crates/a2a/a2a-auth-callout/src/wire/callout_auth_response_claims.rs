use std::fmt;

use nats_jwt_rs::authorization::AuthResponse;

use super::{NATS_JWT_PREFIX, ServerAuthRequestClaims, XkeyPublic};
use crate::admission_keys::{AccountSigningKey, CalloutXkey};
use crate::error::AuthCalloutError;
use crate::jwt::MintedUserJwt;

/// Callout-signed authorization **response** JWT for `$SYS.REQ.USER.AUTH` reply.
#[derive(Clone)]
pub struct CalloutAuthResponseClaims {
    encoded: String,
}

/// `encoded` carries the minted user JWT inside `nats.jwt`, so printing it
/// would put a usable credential wherever the response is traced.
impl fmt::Debug for CalloutAuthResponseClaims {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CalloutAuthResponseClaims")
            .field("encoded", &"<redacted>")
            .finish()
    }
}

impl CalloutAuthResponseClaims {
    pub fn success(
        request: &ServerAuthRequestClaims,
        user_jwt: &MintedUserJwt,
        response_signing_key: &AccountSigningKey,
    ) -> Result<Self, AuthCalloutError> {
        let user_nkey = request.user_nkey()?;
        let mut claim = AuthResponse::generic_claim(user_nkey.as_str().to_owned());
        claim.nats.jwt = user_jwt.as_str().to_owned();
        claim.aud = Some(request.server_id().to_owned());
        let encoded = claim
            .encode(response_signing_key.keypair())
            .map_err(|e| AuthCalloutError::WireFormat(format!("encode authorization response: {e}")))?;
        Ok(Self { encoded })
    }

    pub fn denial(
        request: &ServerAuthRequestClaims,
        message: impl Into<String>,
        response_signing_key: &AccountSigningKey,
    ) -> Result<Self, AuthCalloutError> {
        let user_nkey = request.user_nkey()?;
        let mut claim = AuthResponse::generic_claim(user_nkey.as_str().to_owned());
        claim.nats.error = message.into();
        claim.aud = Some(request.server_id().to_owned());
        let encoded = claim
            .encode(response_signing_key.keypair())
            .map_err(|e| AuthCalloutError::WireFormat(format!("encode authorization denial: {e}")))?;
        Ok(Self { encoded })
    }

    pub fn as_jwt_str(&self) -> &str {
        &self.encoded
    }

    pub fn into_wire_bytes(
        self,
        request: &ServerAuthRequestClaims,
        callout_xkey: Option<&CalloutXkey>,
    ) -> Result<Vec<u8>, AuthCalloutError> {
        let jwt = self.encoded.into_bytes();
        let Some(server_xkey) = request.server_one_time_xkey() else {
            return Ok(jwt);
        };
        let callout_xkey = callout_xkey.ok_or_else(|| {
            AuthCalloutError::WireFormat(
                "server requested encrypted response (server_id.xkey) but the callout xkey is not configured".into(),
            )
        })?;
        let recipient = XkeyPublic::parse(server_xkey)?;
        if jwt.starts_with(NATS_JWT_PREFIX) {
            callout_xkey
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
