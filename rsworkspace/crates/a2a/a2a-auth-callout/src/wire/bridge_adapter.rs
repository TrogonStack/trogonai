//! Converts internal JSON bridge mint requests into [`ServerAuthRequestClaims`]
//! for reuse of [`crate::dispatcher::CalloutDispatcher`].

use nats_jwt_rs::authorization::{AuthRequest, ClientInfo, ClientTLS, ConnectOpts, ServerID};
use nats_jwt_rs::types::GenericFields;
use nats_jwt_rs::{ClaimType, Claims};
use serde::Serialize;

use super::ServerAuthRequestClaims;
use crate::account_resolver::RequestedAccount;
use crate::bridge_mint::{BridgeAuthScheme, BridgeMintRequest};
use crate::error::{AuthCalloutError, CredentialError};

/// The `client_tls` section of a synthetic bridge request. [`ClientTLS`] keeps
/// its fields private, so the bridge states its own view of the section and
/// converts through serde.
#[derive(Debug, Serialize)]
struct BridgeClientTls {
    version: &'static str,
    cipher: &'static str,
    certs: Vec<String>,
    verified_chains: Vec<Vec<String>>,
}

impl BridgeClientTls {
    fn into_client_tls(self) -> Result<ClientTLS, AuthCalloutError> {
        let value =
            serde_json::to_value(self).map_err(|e| AuthCalloutError::WireFormat(format!("bridge client_tls: {e}")))?;
        serde_json::from_value(value).map_err(|e| AuthCalloutError::WireFormat(format!("bridge client_tls: {e}")))
    }
}

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
            .ok_or_else(|| CredentialError::InvalidRequest("bridge mint request missing account".into()))?;
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
                        CredentialError::InvalidRequest(
                            "API-key scheme but connect_opts.api_key is missing or blank".into(),
                        )
                    })?;
                (None, Some(key))
            }
            Some(BridgeAuthScheme::Oidc) => {
                // OIDC must carry a user_jwt — otherwise dispatcher's
                // material-based inference would fall back to mTLS if a
                // client_cert_pem happens to be present, silently
                // authenticating the connection via a different scheme
                // than the caller declared.
                let jwt = request
                    .user_jwt
                    .clone()
                    .filter(|j| !j.trim().is_empty())
                    .ok_or_else(|| {
                        CredentialError::InvalidRequest("OIDC scheme but user_jwt is missing or blank".into())
                    })?;
                (Some(jwt), None)
            }
            None => (request.user_jwt.clone(), None),
            Some(BridgeAuthScheme::MTls) => (None, None),
        };

        let client_tls = request
            .client_info
            .as_ref()
            .and_then(|c| c.client_cert_pem.clone())
            .map(|pem| {
                BridgeClientTls {
                    version: "1.3",
                    cipher: "bridge-internal",
                    certs: vec![pem],
                    verified_chains: Vec::new(),
                }
                .into_client_tls()
            })
            .transpose()?;

        let nats = AuthRequest {
            server: ServerID {
                name: "bridge-internal".to_owned(),
                host: "127.0.0.1".to_owned(),
                id: "BRIDGEINTERNALAUTHCALLOUT000000000000".to_owned(),
                ..ServerID::default()
            },
            user_nkey: request
                .user_nkey
                .unwrap_or_else(|| nkeys::KeyPair::new_user().public_key()),
            client_info: ClientInfo {
                host: "127.0.0.1".to_owned(),
                id: 0,
                user: account,
                kind: "Client".to_owned(),
                client_type: "nats".to_owned(),
                ..ClientInfo::default()
            },
            connect_opts: ConnectOpts {
                jwt,
                auth_token,
                protocol: 1,
                ..ConnectOpts::default()
            },
            client_tls,
            request_nonce: None,
            generic_fields: GenericFields {
                claim_type: ClaimType::AuthorizationRequest,
                version: 2,
                ..GenericFields::default()
            },
        };

        // `ServerAuthRequestClaims` reads the sections `AuthRequest` keeps
        // private (the TLS certificates) off the raw `nats` document, so the
        // synthetic request carries the same projection a server-signed one
        // would.
        let nats_json = serde_json::to_value(&nats)
            .map_err(|e| AuthCalloutError::WireFormat(format!("bridge mint claims: {e}")))?;

        let bridge_issuer = nkeys::KeyPair::new_account();
        let inner: Claims<AuthRequest> = Claims {
            aud: Some(super::AUTH_REQUEST_AUDIENCE.to_owned()),
            exp: None,
            iat: 1,
            id: None,
            iss: bridge_issuer.public_key(),
            jti: "bridge".to_owned(),
            name: None,
            nats,
            nbf: None,
            sub: bridge_issuer.public_key(),
        };

        Ok(Self::from_decoded(inner, nats_json))
    }
}

#[cfg(test)]
mod tests;
