// TODO(spec): The exact wire encoding of $SYS.REQ.USER.AUTH request/reply payloads
// is defined by the NATS server version and auth callout extension.
// Reference: https://docs.nats.io/running-a-nats-service/configuration/securing_nats/auth_callout
//
// The fields below are derived from the illustrative shape in
// docs/A2A_AUTH_CALLOUT_SKETCH.md §2. Replace with the actual nkeys/jwt-encoded
// request struct once the NATS auth callout wire format is pinned for this deployment.

use serde::{Deserialize, Serialize};

use crate::error::AuthCalloutError;

/// Illustrative shape of the auth callout request published by the NATS server
/// on `$SYS.REQ.USER.AUTH`.
///
/// Field names follow the sketch in `docs/A2A_AUTH_CALLOUT_SKETCH.md` §2.
/// The real server payload may be NKey-signed and/or JWT-encoded; replace this
/// struct once the wire format is confirmed for the target NATS operator setup.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthCalloutRequest {
    /// Client-supplied credential material (NKey public key or JWT bearer).
    pub user_nkey: Option<String>,
    /// Client-supplied JWT if the connect options included a token.
    pub user_jwt: Option<String>,
    /// Target Account the client is attempting to join.
    pub account: Option<String>,
    /// Opaque client connection metadata (TLS state, IP, client library id).
    pub client_info: Option<serde_json::Value>,
    /// Optional tags or headers from the client connect options.
    pub connect_opts: Option<serde_json::Value>,
}

/// Illustrative shape of a successful auth callout reply (callout → server).
///
/// On success the service must return a signed User JWT bound to the tenant Account.
/// On failure it must return an authorization denied indicator — represented here as
/// an `Err` from the `AuthDispatcher`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthCalloutResponse {
    /// Signed User JWT bound to the tenant Account (short TTL).
    pub user_jwt: String,
}

/// Trait that the subscriber loop delegates auth decisions to.
///
/// Inject a test double via `Subscriber::new` to unit-test the loop without a
/// live NATS connection.
#[async_trait::async_trait]
pub trait AuthDispatcher: Send + Sync + 'static {
    async fn dispatch(&self, request: AuthCalloutRequest) -> Result<AuthCalloutResponse, AuthCalloutError>;
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub struct AlwaysDenyDispatcher;

    #[async_trait::async_trait]
    impl AuthDispatcher for AlwaysDenyDispatcher {
        async fn dispatch(&self, _request: AuthCalloutRequest) -> Result<AuthCalloutResponse, AuthCalloutError> {
            Err(AuthCalloutError::CredentialVerification("stub: always deny".into()))
        }
    }

    #[tokio::test]
    async fn always_deny_returns_err() {
        let d = AlwaysDenyDispatcher;
        let req = AuthCalloutRequest {
            user_nkey: None,
            user_jwt: None,
            account: None,
            client_info: None,
            connect_opts: None,
        };
        assert!(d.dispatch(req).await.is_err());
    }
}
