use std::sync::Arc;
use std::time::{Duration, SystemTime};

use futures::StreamExt as _;
use tracing::{error, info, warn};

use crate::denial_category::DenialCategory;
use crate::denial_claims::{CalloutIssuer, DenialClaims, ServerAudience, UserNkeySubject};
use crate::denial_reason::DenialReason;
use crate::dispatcher::{AuthCalloutRequest, AuthCalloutResponse, AuthDispatcher};
use crate::error::AuthCalloutError;
use crate::jwt::SigningKey;

/// NATS subject the server uses for auth callout requests.
const AUTH_CALLOUT_SUBJECT: &str = "$SYS.REQ.USER.AUTH";

const DEFAULT_DENIAL_TTL: Duration = Duration::from_secs(60);

/// Signing and issuer material for NATS authorization-response denial JWTs.
pub struct DenialPublisherConfig {
    pub signing_key: SigningKey,
    pub issuer: CalloutIssuer,
    pub denial_ttl: Duration,
}

impl DenialPublisherConfig {
    pub fn new(signing_key: SigningKey, issuer: CalloutIssuer) -> Self {
        Self {
            signing_key,
            issuer,
            denial_ttl: DEFAULT_DENIAL_TTL,
        }
    }
}

/// Subscriber loop that listens on `$SYS.REQ.USER.AUTH` and delegates each
/// request to the injected `AuthDispatcher`.
///
/// Keeping I/O (NATS) and decision logic (dispatcher) separate makes the loop
/// unit-testable without a live server.
pub struct Subscriber<D> {
    client: async_nats::Client,
    dispatcher: Arc<D>,
    denial: Arc<DenialPublisherConfig>,
}

impl<D: AuthDispatcher> Subscriber<D> {
    pub fn new(client: async_nats::Client, dispatcher: D, denial: DenialPublisherConfig) -> Self {
        Self {
            client,
            dispatcher: Arc::new(dispatcher),
            denial: Arc::new(denial),
        }
    }

    pub async fn run(self) -> Result<(), AuthCalloutError> {
        let mut sub = self
            .client
            .subscribe(AUTH_CALLOUT_SUBJECT)
            .await
            .map_err(|e| AuthCalloutError::Subscribe(e.to_string()))?;

        info!(subject = AUTH_CALLOUT_SUBJECT, "auth callout subscriber started");

        while let Some(msg) = sub.next().await {
            let reply = match msg.reply.clone() {
                Some(r) => r,
                None => {
                    warn!("auth callout request without reply subject; dropping");
                    continue;
                }
            };

            let request: AuthCalloutRequest = match serde_json::from_slice(&msg.payload) {
                Ok(r) => r,
                Err(e) => {
                    warn!(error = %e, "failed to deserialize auth callout request; dropping");
                    continue;
                }
            };

            let dispatcher = Arc::clone(&self.dispatcher);
            let client = self.client.clone();
            let denial = self.denial.clone();

            tokio::spawn(async move {
                match dispatcher.dispatch(request.clone()).await {
                    Ok(response) => {
                        if let Err(e) = publish_response(&client, &reply, &response).await {
                            error!(error = %e, "failed to publish auth callout response");
                        }
                    }
                    Err(e) => {
                        publish_denial(&client, &reply, &request, &e, &denial).await;
                    }
                }
            });
        }

        warn!("auth callout NATS subscription closed");
        Ok(())
    }
}

async fn publish_response(
    client: &async_nats::Client,
    reply: &str,
    response: &AuthCalloutResponse,
) -> Result<(), AuthCalloutError> {
    let payload = serde_json::to_vec(response).map_err(AuthCalloutError::Serialize)?;
    client
        .publish(reply.to_string(), payload.into())
        .await
        .map_err(|e| AuthCalloutError::Reply(e.to_string()))
}

async fn publish_denial(
    client: &async_nats::Client,
    reply: &str,
    request: &AuthCalloutRequest,
    error: &AuthCalloutError,
    config: &Arc<DenialPublisherConfig>,
) {
    let category = DenialCategory::from_auth_callout_error(error);
    let caller_id_hint = caller_id_hint(request);
    let request_jti = request.request_jti.as_deref().unwrap_or("");

    warn!(
        reason_category = category.as_str(),
        request_jti,
        caller_id_hint = caller_id_hint.as_deref().unwrap_or(""),
        error = %error,
        "auth callout denied"
    );

    let payload = match encode_denial_jwt(request, category, config.as_ref()) {
        Ok(bytes) => bytes,
        Err(e) => {
            error!(error = %e, "failed to encode auth callout denial JWT");
            return;
        }
    };

    if let Err(e) = client.publish(reply.to_string(), payload.into()).await {
        error!(error = %e, "failed to publish auth callout denial");
    }
}

fn encode_denial_jwt(
    request: &AuthCalloutRequest,
    category: DenialCategory,
    config: &DenialPublisherConfig,
) -> Result<Vec<u8>, AuthCalloutError> {
    let server_id = request
        .server_id
        .as_deref()
        .ok_or_else(|| AuthCalloutError::JwtMint("auth request missing server_id for denial aud".into()))?;
    let user_nkey = request
        .user_nkey
        .as_deref()
        .ok_or_else(|| AuthCalloutError::JwtMint("auth request missing user_nkey for denial sub".into()))?;

    let claims = DenialClaims {
        iss: config.issuer.clone(),
        aud: ServerAudience::new(server_id).map_err(|e| AuthCalloutError::JwtMint(e.to_string()))?,
        sub: UserNkeySubject::new(user_nkey).map_err(|e| AuthCalloutError::JwtMint(e.to_string()))?,
        reason: DenialReason::new(category).map_err(|e| AuthCalloutError::JwtMint(e.to_string()))?,
        request_jti: request.request_jti.clone(),
    };

    let jwt = claims
        .mint(&config.signing_key, SystemTime::now(), config.denial_ttl)
        .map_err(AuthCalloutError::from)?;

    Ok(jwt.into_bytes())
}

fn caller_id_hint(request: &AuthCalloutRequest) -> Option<String> {
    request
        .user_nkey
        .clone()
        .or_else(|| request.account.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dispatcher::tests::AlwaysDenyDispatcher;

    fn test_denial_config() -> DenialPublisherConfig {
        DenialPublisherConfig::new(
            SigningKey::from_secret(b"subscriber-denial-test-secret---"),
            CalloutIssuer::new("ACALLOUTISSUER").unwrap(),
        )
    }

    fn sample_request() -> AuthCalloutRequest {
        AuthCalloutRequest {
            user_nkey: Some("UCLIENTNKEY".into()),
            user_jwt: None,
            account: Some("tenant-acme".into()),
            client_info: None,
            connect_opts: None,
            server_id: Some("ASERVERPUBKEY".into()),
            request_jti: Some("REQJTI".into()),
        }
    }

    #[test]
    fn encode_denial_jwt_has_nats_error_not_raw_json() {
        let err = AuthCalloutError::CredentialVerification("OIDC token validation failed: bad sig".into());
        let bytes = encode_denial_jwt(&sample_request(), DenialCategory::from_auth_callout_error(&err), &test_denial_config())
            .unwrap();
        let token = std::str::from_utf8(&bytes).unwrap();
        assert_eq!(token.split('.').count(), 3);
        assert!(!token.contains("bad sig"));
        assert!(!token.contains("authorization denied"));
    }

    #[test]
    fn encode_denial_requires_server_id() {
        let mut req = sample_request();
        req.server_id = None;
        let err = encode_denial_jwt(
            &req,
            DenialCategory::InvalidCredentials,
            &test_denial_config(),
        );
        assert!(err.is_err());
    }

    #[test]
    fn caller_id_hint_prefers_user_nkey() {
        let req = sample_request();
        assert_eq!(caller_id_hint(&req).as_deref(), Some("UCLIENTNKEY"));
    }

    #[test]
    fn denial_category_from_always_deny_dispatcher_error() {
        let err = AuthCalloutError::CredentialVerification("stub: always deny".into());
        assert_eq!(
            DenialCategory::from_auth_callout_error(&err),
            DenialCategory::InvalidCredentials
        );
    }

    #[tokio::test]
    async fn always_deny_dispatcher_still_denies() {
        let d = AlwaysDenyDispatcher;
        let req = sample_request();
        assert!(d.dispatch(req).await.is_err());
    }
}
