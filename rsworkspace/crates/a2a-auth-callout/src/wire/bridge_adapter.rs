//! Converts internal JSON bridge mint requests into [`ServerAuthRequestClaims`]
//! for reuse of [`crate::dispatcher::CalloutDispatcher`].

use nats_jwt_rs::Claims;
use nats_jwt_rs::authorization::AuthRequest;

use super::ServerAuthRequestClaims;
use crate::account_resolver::RequestedAccount;
use crate::bridge_mint::{BridgeAuthScheme, BridgeMintRequest};
use crate::error::AuthCalloutError;

impl ServerAuthRequestClaims {
    /// Synthetic authorization claims for `a2a.bridge.auth.callout.request` (not server-signed).
    pub fn from_bridge_mint(request: BridgeMintRequest) -> Result<Self, AuthCalloutError> {
        // Route the bridge account through the same RequestedAccount value
        // object the dispatcher uses for the NATS path — keeps the shape
        // consistent and means a blank/whitespace account is rejected with
        // a typed AccountResolverError instead of slipping through and
        // failing later during dispatch.
        let raw_account = request
            .account
            .ok_or_else(|| AuthCalloutError::CredentialVerification("bridge mint request missing account".into()))?;
        // The validated RequestedAccount is the boundary type; we keep the
        // string form for the synthetic JSON body but the validation has
        // happened by the time it gets here.
        let account = RequestedAccount::new(raw_account.trim())?.as_str().to_owned();

        let (jwt, auth_token) = match request.connect_opts.as_ref().and_then(|o| o.auth_scheme) {
            Some(BridgeAuthScheme::ApiKey) => {
                let key = request
                    .connect_opts
                    .as_ref()
                    .and_then(|o| o.api_key.clone())
                    .filter(|k| !k.trim().is_empty())
                    .ok_or_else(|| {
                        AuthCalloutError::CredentialVerification(
                            "API-key scheme but connect_opts.api_key is missing or blank".into(),
                        )
                    })?;
                (None, Some(key))
            }
            Some(BridgeAuthScheme::Oidc) | None => (request.user_jwt.clone(), None),
            Some(BridgeAuthScheme::MTls) => (None, None),
        };

        let client_tls = request
            .client_info
            .as_ref()
            .and_then(|c| c.client_cert_pem.clone())
            .map(|pem| {
                serde_json::json!({
                    "version": "1.3",
                    "cipher": "bridge-internal",
                    "certs": [pem],
                    "verified_chains": []
                })
            });

        let nats = serde_json::json!({
            "server_id": {
                "name": "bridge-internal",
                "host": "127.0.0.1",
                "id": "BRIDGEINTERNALAUTHCALLOUT000000000000"
            },
            "user_nkey": request
                .user_nkey
                .unwrap_or_else(|| nkeys::KeyPair::new_user().public_key()),
            "client_info": {
                "host": "127.0.0.1",
                "id": 0,
                "user": account,
                "name_tag": "",
                "kind": "Client",
                "type": "nats",
                "nonce": ""
            },
            "connect_opts": {
                "jwt": jwt,
                "auth_token": auth_token,
                "protocol": 1
            },
            "client_tls": client_tls,
            "type": "authorization_request",
            "version": 2
        });

        let bridge_issuer = nkeys::KeyPair::new_account();
        let nats_json = nats.clone();
        let inner: Claims<AuthRequest> = serde_json::from_value(serde_json::json!({
            "aud": super::AUTH_REQUEST_AUDIENCE,
            "iat": 1,
            "iss": bridge_issuer.public_key(),
            "jti": "bridge",
            "sub": bridge_issuer.public_key(),
            "nats": nats
        }))
        .map_err(|e| AuthCalloutError::WireFormat(format!("bridge mint claims: {e}")))?;

        Ok(Self::from_decoded(inner, nats_json))
    }
}
