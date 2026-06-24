use nkeys::{KeyPair, XKey};

use super::{
    CalloutAuthResponseClaims, NkeyPublic, NkeySeed, ServerAuthRequestClaims, ServerAuthRequestEnvelope, XkeyPublic,
};
use crate::error::AuthCalloutError;
use crate::jwt::MintedUserJwt;

/// Configuration for NATS `$SYS.REQ.USER.AUTH` encode/decode.
pub struct AuthCalloutWireCodec {
    server_issuer: NkeyPublic,
    callout_issuer: KeyPair,
    account_xkey_seed: Option<NkeySeed>,
    account_xkey: Option<XKey>,
    server_xkey_public: Option<XkeyPublic>,
}

impl AuthCalloutWireCodec {
    pub fn new(
        server_issuer: NkeyPublic,
        callout_issuer_seed: NkeySeed,
        account_xkey_seed: Option<NkeySeed>,
        server_xkey_public: Option<XkeyPublic>,
    ) -> Result<Self, AuthCalloutError> {
        // XKey encryption is symmetric in shape: account_xkey_seed lets the
        // callout decrypt server-encrypted requests; server_xkey_public is the
        // pinned identity of the server's encryption key for encrypt-of-the-
        // response or fallback decrypt. Configuring only one side is a foot-
        // gun — requests stay readable but responses won't encrypt, or vice
        // versa. Fail fast instead of silently half-enabling encryption.
        match (account_xkey_seed.as_ref(), server_xkey_public.as_ref()) {
            (Some(_), None) | (None, Some(_)) => {
                return Err(AuthCalloutError::WireFormat(
                    "AuthCalloutWireCodec XKey config is partial: account_xkey_seed and \
                     server_xkey_public must both be set or both unset"
                        .into(),
                ));
            }
            _ => {}
        }
        let callout_issuer = callout_issuer_seed.to_signing_keypair()?;
        let account_xkey = account_xkey_seed.as_ref().map(NkeySeed::to_xkey).transpose()?;
        Ok(Self {
            server_issuer,
            callout_issuer,
            account_xkey_seed,
            account_xkey,
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
            self.account_xkey_seed.as_ref(),
            self.server_xkey_public.as_ref(),
        )
    }

    pub fn encode_success(
        &self,
        request: &ServerAuthRequestClaims,
        user_jwt: MintedUserJwt,
    ) -> Result<Vec<u8>, AuthCalloutError> {
        let response = CalloutAuthResponseClaims::success(request, &user_jwt, &self.callout_issuer)?;
        response.into_wire_bytes(request, self.account_xkey.as_ref())
    }

    pub fn encode_denial(
        &self,
        request: &ServerAuthRequestClaims,
        message: impl Into<String>,
    ) -> Result<Vec<u8>, AuthCalloutError> {
        let response = CalloutAuthResponseClaims::denial(request, message, &self.callout_issuer)?;
        response.into_wire_bytes(request, self.account_xkey.as_ref())
    }
}

impl std::fmt::Debug for AuthCalloutWireCodec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AuthCalloutWireCodec")
            .field("server_issuer", &self.server_issuer)
            .field("callout_issuer", &self.callout_issuer.public_key())
            .field("account_xkey", &self.account_xkey.is_some())
            .finish()
    }
}

#[cfg(test)]
mod tests;
