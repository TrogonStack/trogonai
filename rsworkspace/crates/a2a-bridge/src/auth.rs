//! Auth callout client surface wired to HTTPS termination.

use async_trait::async_trait;

use crate::error::BridgeError;
use crate::identity::{BridgeUserJwt, CallerHttpsAuth};

#[async_trait]
pub trait AuthCalloutClient: Send + Sync {
    async fn mint(&self, caller_auth: &CallerHttpsAuth) -> Result<BridgeUserJwt, BridgeError>;
}

#[derive(Clone, Copy, Debug, Default)]
pub struct StubAuthCalloutClient;

#[async_trait]
impl AuthCalloutClient for StubAuthCalloutClient {
    async fn mint(&self, _caller_auth: &CallerHttpsAuth) -> Result<BridgeUserJwt, BridgeError> {
        // TODO(auth-callout-client): Replace with NATS-backed client once `a2a-auth-callout` exposes request API for minting.
        unimplemented!("wired when a2a-auth-callout exposes a request API")
    }
}
