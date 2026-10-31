use super::{CalloutAuthResponseClaims, NkeyPublic, ServerAuthRequestClaims, ServerAuthRequestEnvelope, XkeyPublic};
use crate::admission_keys::{AccountSigningKey, CalloutXkey};
use crate::error::AuthCalloutError;
use crate::jwt::MintedUserJwt;

/// Configuration for NATS `$SYS.REQ.USER.AUTH` encode/decode.
pub struct AuthCalloutWireCodec {
    server_issuer: NkeyPublic,
    response_signing_key: AccountSigningKey,
    callout_xkey: Option<CalloutXkey>,
    server_xkey_public: Option<XkeyPublic>,
}

impl AuthCalloutWireCodec {
    pub fn new(
        server_issuer: NkeyPublic,
        response_signing_key: AccountSigningKey,
        callout_xkey: Option<CalloutXkey>,
        server_xkey_public: Option<XkeyPublic>,
    ) -> Result<Self, AuthCalloutError> {
        if callout_xkey.is_some() != server_xkey_public.is_some() {
            return Err(AuthCalloutError::WireFormat(
                "AuthCalloutWireCodec XKey config is partial: the callout xkey and server_xkey_public must both be \
                 set or both unset"
                    .into(),
            ));
        }
        Ok(Self {
            server_issuer,
            response_signing_key,
            callout_xkey,
            server_xkey_public,
        })
    }

    pub fn decode_request(
        &self,
        payload: Vec<u8>,
        headers: Option<&async_nats::HeaderMap>,
    ) -> Result<ServerAuthRequestClaims, AuthCalloutError> {
        ServerAuthRequestEnvelope::decode_from_message(
            payload,
            headers,
            &self.server_issuer,
            self.callout_xkey.as_ref(),
            self.server_xkey_public.as_ref(),
        )
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
            .field("server_issuer", &self.server_issuer)
            .field("response_signing_key", &self.response_signing_key)
            .field("callout_xkey", &self.callout_xkey)
            .finish()
    }
}

#[cfg(test)]
mod tests;
