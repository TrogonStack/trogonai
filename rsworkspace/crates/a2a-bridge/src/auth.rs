use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::json;
use tokio::time::timeout;

use a2a_auth_callout::{AuthCalloutRequest, AuthCalloutResponse};

use crate::error::BridgeError;
use crate::identity::{BridgeUserJwt, CallerHttpsAuth};

#[async_trait]
pub trait AuthCalloutClient: Send + Sync {
    async fn mint(&self, caller_auth: &CallerHttpsAuth) -> Result<BridgeUserJwt, BridgeError>;
}

#[derive(Clone)]
pub struct StubAuthCalloutClient;

#[async_trait]
impl AuthCalloutClient for StubAuthCalloutClient {
    async fn mint(&self, _caller_auth: &CallerHttpsAuth) -> Result<BridgeUserJwt, BridgeError> {
        Err(BridgeError::Mint(
            "auth callout not configured for this deployment".into(),
        ))
    }
}

#[async_trait]
pub trait AuthMintWire: Send + Sync {
    async fn roundtrip_message(&self, subject: String, payload: BytesPayload) -> Result<Vec<u8>, BridgeError>;
}

#[derive(Clone, Debug)]
pub struct BytesPayload(pub Vec<u8>);

#[derive(Clone)]
pub struct AsyncNatsAuthMintWire {
    client: Arc<async_nats::Client>,
    request_timeout: Duration,
}

impl AsyncNatsAuthMintWire {
    pub fn new(client: Arc<async_nats::Client>, request_timeout: Duration) -> Self {
        Self {
            client,
            request_timeout,
        }
    }

    #[must_use]
    pub fn from_client(client: async_nats::Client, request_timeout: Duration) -> Self {
        Self::new(Arc::new(client), request_timeout)
    }
}

#[async_trait]
impl AuthMintWire for AsyncNatsAuthMintWire {
    async fn roundtrip_message(&self, subject: String, payload: BytesPayload) -> Result<Vec<u8>, BridgeError> {
        tokio::time::timeout(self.request_timeout, self.client.request(subject, payload.0.into()))
            .await
            .map_err(|_| BridgeError::Mint(
                "auth mint NATS roundtrip exceeded timeout".into(),
            ))?
            .map_err(|e| BridgeError::Mint(e.to_string()))
            .map(|m| m.payload.to_vec())
    }
}

pub struct AuthCalloutJsonMintClient<W: AuthMintWire> {
    wire: Arc<W>,
    mint_subject: Arc<str>,
}

impl<W: AuthMintWire + 'static> AuthCalloutJsonMintClient<W> {
    #[must_use]
    pub fn new(wire: Arc<W>, mint_subject: impl Into<Arc<str>>) -> Self {
        Self {
            wire,
            mint_subject: mint_subject.into(),
        }
    }

    #[must_use]
    pub fn default_mint_subject() -> &'static str {
        "a2a.bridge.auth.callout.request"
    }
}

#[async_trait]
impl<W: AuthMintWire + 'static> AuthCalloutClient for AuthCalloutJsonMintClient<W> {
    async fn mint(&self, caller_auth: &CallerHttpsAuth) -> Result<BridgeUserJwt, BridgeError> {
        let envelope = AuthCalloutRequest {
            user_nkey: None,
            user_jwt: None,
            account: None,
            client_info: Some(json!({
                "https_authorization_header": caller_auth.as_str(),
            })),
            connect_opts: Some(json!({ "ingress": "a2a-bridge_https" })),
        };
        let bytes =
            serde_json::to_vec(&envelope).map_err(|e: serde_json::Error| BridgeError::Serialize(e))?;
        let mint = self.wire.roundtrip_message(self.mint_subject.to_string(), BytesPayload(bytes));
        let reply = timeout(Duration::from_secs(30), mint)
            .await
            .map_err(|_| BridgeError::Mint("auth mint client deadline exceeded".into()))??;

        let response: AuthCalloutResponse =
            serde_json::from_slice(&reply).map_err(|e: serde_json::Error| BridgeError::Deserialize(e))?;
        Ok(BridgeUserJwt::new(response.user_jwt))
    }
}
