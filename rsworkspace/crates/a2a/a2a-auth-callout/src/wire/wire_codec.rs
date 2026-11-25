use super::{AuthRequestError, CalloutAuthResponseClaims, ServerAuthRequestClaims, ServerAuthRequestEnvelope};
use crate::admission_keys::{AccountSigningKey, AuthAccount, CalloutXkey};
use crate::error::AuthCalloutError;
use crate::jwt::MintedUserJwt;

/// Configuration for NATS `$SYS.REQ.USER.AUTH` encode/decode.
pub struct AuthCalloutWireCodec {
    auth_account: AuthAccount,
    response_signing_key: AccountSigningKey,
    callout_xkey: Option<CalloutXkey>,
}

impl AuthCalloutWireCodec {
    pub fn new(
        auth_account: AuthAccount,
        response_signing_key: AccountSigningKey,
        callout_xkey: Option<CalloutXkey>,
    ) -> Self {
        Self {
            auth_account,
            response_signing_key,
            callout_xkey,
        }
    }

    pub fn decode_request(
        &self,
        payload: Vec<u8>,
        headers: Option<&async_nats::HeaderMap>,
    ) -> Result<ServerAuthRequestClaims, AuthRequestError> {
        ServerAuthRequestEnvelope::decode_from_message(payload, headers, &self.auth_account, self.callout_xkey.as_ref())
    }

    pub fn encode_success(
        &self,
        request: &ServerAuthRequestClaims,
        user_jwt: MintedUserJwt,
    ) -> Result<Vec<u8>, AuthCalloutError> {
        let response = CalloutAuthResponseClaims::success(request, &user_jwt, &self.response_signing_key)?;
        response.into_wire_bytes(request, self.callout_xkey.as_ref())
    }

    pub fn encode_denial(
        &self,
        request: &ServerAuthRequestClaims,
        message: impl Into<String>,
    ) -> Result<Vec<u8>, AuthCalloutError> {
        let response = CalloutAuthResponseClaims::denial(request, message, &self.response_signing_key)?;
        response.into_wire_bytes(request, self.callout_xkey.as_ref())
    }
}

impl std::fmt::Debug for AuthCalloutWireCodec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AuthCalloutWireCodec")
            .field("auth_account", &self.auth_account)
            .field("response_signing_key", &self.response_signing_key)
            .field("callout_xkey", &self.callout_xkey)
            .finish()
    }
}

#[cfg(test)]
mod tests;
