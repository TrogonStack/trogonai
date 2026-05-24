use nats_jwt_rs::authorization::AuthResponse;
use nkeys::{KeyPair, XKey};

use super::{ServerAuthRequestClaims, NATS_JWT_PREFIX};
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
        let recipient = XKey::from_public_key(server_xkey_pub).map_err(|e| {
            AuthCalloutError::WireFormat(format!("invalid server one-time XKey in request: {e}"))
        })?;
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
mod tests {
    use base64::Engine as _;

    use super::*;
    use nkeys::KeyPair;

    use crate::wire::test_encode::signed_auth_request;
    use crate::wire::{NkeyPublic, ServerAuthRequestEnvelope};

    fn fixture_request() -> (ServerAuthRequestClaims, KeyPair) {
        let server = KeyPair::new_account();
        let callout = KeyPair::new_account();
        let user = KeyPair::new_user();
        let token = signed_auth_request(&server, &user, |c| {
            c.nats.connect_opts.jwt = None;
        });
        let server_pub = NkeyPublic::parse(server.public_key()).unwrap();
        let decoded = ServerAuthRequestEnvelope::from_bytes(token.into_bytes())
            .decode(&server_pub, None, None)
            .unwrap();
        (decoded, callout)
    }

    #[test]
    fn success_response_roundtrip_fields() {
        let (req, callout) = fixture_request();
        let resp = CalloutAuthResponseClaims::success(
            &req,
            &MintedUserJwt::new("eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiJ4In0.c2ln"),
            &callout,
        )
        .unwrap();
        // `nats-jwt-rs` omits empty `nats.error` on encode; Go accepts that, but Rust decode requires the field.
        let parts: Vec<&str> = resp.as_jwt_str().split('.').collect();
        assert_eq!(parts.len(), 3);
        let payload_bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(parts[1])
            .expect("response JWT payload base64");
        let payload: serde_json::Value =
            serde_json::from_slice(&payload_bytes).expect("response JWT payload JSON");
        assert_eq!(payload["sub"], req.user_nkey().unwrap().as_str());
        assert_eq!(payload["aud"], req.server_id());
        assert_eq!(payload["nats"]["jwt"].as_str().unwrap(), "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiJ4In0.c2ln");
        assert!(
            payload["nats"]["error"].as_str().unwrap_or("").is_empty(),
            "success response must not set nats.error"
        );
    }

    #[test]
    fn denial_response_carries_error_claim() {
        let (req, callout) = fixture_request();
        let resp =
            CalloutAuthResponseClaims::denial(&req, "authorization denied", &callout).unwrap();
        let decoded =
            nats_jwt_rs::Claims::<AuthResponse>::decode(resp.as_jwt_str()).unwrap();
        assert_eq!(decoded.nats.error, "authorization denied");
        assert!(decoded.nats.jwt.is_empty());
    }
}
