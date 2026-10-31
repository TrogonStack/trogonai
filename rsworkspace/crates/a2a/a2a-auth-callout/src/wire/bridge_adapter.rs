//! Converts internal JSON bridge mint requests into [`ServerAuthRequestClaims`]
//! for reuse of [`crate::dispatcher::CalloutDispatcher`].

use nats_jwt_rs::authorization::{AuthRequest, ClientInfo, ClientTLS, ConnectOpts, ServerID};
use nats_jwt_rs::types::GenericFields;
use nats_jwt_rs::{ClaimType, Claims};
use serde::Serialize;
use serde::de::DeserializeOwned;

use super::ServerAuthRequestClaims;
use crate::account_resolver::RequestedAccount;
use crate::bridge_mint::{BridgeAuthScheme, BridgeMintRequest};
use crate::credentials::PresentedCredential;
use crate::credentials::api_key::ApiKey;
use crate::credentials::oidc::BearerToken;
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
        decode_section("client_tls", encode_section("client_tls", self)?)
    }
}

/// A section the bridge authors, as the JSON document the wire types are
/// defined over.
fn encode_section(section: &'static str, value: impl Serialize) -> Result<serde_json::Value, AuthCalloutError> {
    serde_json::to_value(value).map_err(|source| AuthCalloutError::BridgeWireFormat { section, source })
}

/// The same document read back as the wire type, whose fields are private.
fn decode_section<T: DeserializeOwned>(section: &'static str, value: serde_json::Value) -> Result<T, AuthCalloutError> {
    serde_json::from_value(value).map_err(|source| AuthCalloutError::BridgeWireFormat { section, source })
}

/// The edge knows which scheme its caller used, so the secret it forwards is
/// declared with that scheme instead of being classified by shape.
fn declared_credential(request: &BridgeMintRequest) -> Result<Option<PresentedCredential>, AuthCalloutError> {
    let user_jwt = request
        .user_jwt
        .as_deref()
        .filter(|jwt| !jwt.trim().is_empty())
        .map(|jwt| PresentedCredential::BearerToken(BearerToken::new(jwt)));
    match request.connect_opts.as_ref().and_then(|opts| opts.auth_scheme) {
        Some(BridgeAuthScheme::Oidc) => {
            let token = user_jwt.ok_or_else(|| {
                CredentialError::InvalidRequest("OIDC scheme but user_jwt is missing or blank".into())
            })?;
            Ok(Some(token))
        }
        Some(BridgeAuthScheme::ApiKey) => {
            let key = request
                .connect_opts
                .as_ref()
                .and_then(|opts| opts.api_key.as_deref())
                .filter(|key| !key.trim().is_empty())
                .ok_or_else(|| {
                    CredentialError::InvalidRequest(
                        "API-key scheme but connect_opts.api_key is missing or blank".into(),
                    )
                })?;
            Ok(Some(PresentedCredential::ApiKey(ApiKey::new(key)?)))
        }
        Some(BridgeAuthScheme::MTls) => {
            let has_client_cert = request
                .client_info
                .as_ref()
                .is_some_and(|info| info.client_cert_pem.is_some());
            if !has_client_cert {
                return Err(CredentialError::InvalidRequest(
                    "mTLS scheme but client_info.client_cert_pem is missing".into(),
                )
                .into());
            }
            Ok(None)
        }
        None => Ok(user_jwt),
    }
}

impl ServerAuthRequestClaims {
    /// Synthetic authorization claims for `a2a.bridge.auth.callout.request` (not server-signed).
    pub fn from_bridge_mint(request: BridgeMintRequest) -> Result<Self, AuthCalloutError> {
        let raw_account = request
            .account
            .as_deref()
            .ok_or_else(|| CredentialError::InvalidRequest("bridge mint request missing account".into()))?;
        let account = RequestedAccount::new(raw_account.trim())?;
        let declared_credential = declared_credential(&request)?;

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
                kind: "Client".to_owned(),
                client_type: "nats".to_owned(),
                ..ClientInfo::default()
            },
            connect_opts: ConnectOpts {
                user: Some(account.as_str().to_owned()),
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
        let nats_json = encode_section("nats", &nats)?;

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

        Ok(Self::declaring(inner, nats_json, declared_credential))
    }
}

#[cfg(test)]
mod tests;
