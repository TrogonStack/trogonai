//! STS-backed mesh token minter.
//!
//! Posts form-encoded RFC 8693 token-exchange requests to a
//! `trogon-sts` HTTP front (see `trogon-sts/src/http.rs`) and caches
//! the resulting access token by `(audience, tenant)` until shortly
//! before expiry, mirroring the egress mint cache in
//! `trogon-mcp-gateway::egress::mint`.
//!
//! The proxy still owns *which* subject identity to present — the
//! minter is given a `SubjectProvider` so deployments can plug in
//! their workload identity story (SPIFFE SVID JWT, static service
//! account, etc.) without entangling that decision with the HTTP
//! mint path itself.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use serde::Deserialize;

use crate::proxy::TokenMinter;
use crate::routes::TokenExchangeMode;

const GRANT_TYPE_TOKEN_EXCHANGE: &str = "urn:ietf:params:oauth:grant-type:token-exchange";
const TOKEN_TYPE_JWT: &str = "urn:ietf:params:oauth:token-type:jwt";
/// Renew tokens this many seconds before their `expires_in` so callers
/// never see a freshly-expired bearer on the upstream leg.
const RENEW_LEEWAY_SECS: u64 = 30;

#[async_trait]
pub trait SubjectProvider: Send + Sync {
    /// Return the current subject token (a JWT) representing the
    /// caller principal that the STS should exchange on behalf of.
    async fn subject_token(&self, tenant: &str) -> Result<String, String>;
}

/// Subject provider that always returns the same configured token.
/// Useful for tests and for proxies whose subject is a stable
/// service-account JWT mounted at startup.
pub struct StaticSubjectProvider {
    token: String,
}

impl StaticSubjectProvider {
    pub fn new(token: impl Into<String>) -> Self {
        Self { token: token.into() }
    }
}

#[async_trait]
impl SubjectProvider for StaticSubjectProvider {
    async fn subject_token(&self, _tenant: &str) -> Result<String, String> {
        Ok(self.token.clone())
    }
}

#[derive(Debug, Clone, Deserialize)]
struct StsTokenResponse {
    access_token: String,
    #[serde(default)]
    expires_in: Option<u64>,
}

#[derive(Debug, Clone, Deserialize)]
struct StsErrorResponse {
    error: String,
    #[serde(default)]
    error_description: Option<String>,
}

#[derive(Clone)]
struct CachedToken {
    token: String,
    renew_after: Instant,
}

pub struct StsMinter {
    http: reqwest::Client,
    token_endpoint: String,
    subject: Arc<dyn SubjectProvider>,
    cache: Mutex<HashMap<(String, String), CachedToken>>,
}

impl StsMinter {
    pub fn new(http: reqwest::Client, token_endpoint: impl Into<String>, subject: Arc<dyn SubjectProvider>) -> Self {
        Self {
            http,
            token_endpoint: token_endpoint.into(),
            subject,
            cache: Mutex::new(HashMap::new()),
        }
    }

    fn mode_form_value(mode: TokenExchangeMode) -> &'static str {
        match mode {
            TokenExchangeMode::Delegation => "Delegation",
            TokenExchangeMode::ExchangeOnly => "ExchangeOnly",
            TokenExchangeMode::ElicitationOnly => "ElicitationOnly",
            TokenExchangeMode::AuthOnly => "AuthOnly",
        }
    }

    fn cache_get(&self, key: &(String, String)) -> Option<String> {
        let now = Instant::now();
        let guard = self.cache.lock().unwrap();
        guard
            .get(key)
            .filter(|entry| entry.renew_after > now)
            .map(|entry| entry.token.clone())
    }

    fn cache_put(&self, key: (String, String), token: String, ttl_secs: u64) {
        let leeway = RENEW_LEEWAY_SECS.min(ttl_secs);
        let renew_after = Instant::now() + Duration::from_secs(ttl_secs.saturating_sub(leeway));
        self.cache
            .lock()
            .unwrap()
            .insert(key, CachedToken { token, renew_after });
    }
}

#[async_trait]
impl TokenMinter for StsMinter {
    async fn mint(&self, audience: &str, tenant: &str, mode: TokenExchangeMode) -> Result<String, String> {
        let key = (audience.to_string(), tenant.to_string());
        if let Some(t) = self.cache_get(&key) {
            return Ok(t);
        }

        let subject_token = self.subject.subject_token(tenant).await?;
        let mode_value = Self::mode_form_value(mode);

        let form = [
            ("grant_type", GRANT_TYPE_TOKEN_EXCHANGE),
            ("subject_token", subject_token.as_str()),
            ("subject_token_type", TOKEN_TYPE_JWT),
            ("audience", audience),
            ("mode", mode_value),
        ];

        let resp = self
            .http
            .post(&self.token_endpoint)
            .form(&form)
            .send()
            .await
            .map_err(|e| format!("sts request: {e}"))?;

        let status = resp.status();
        let bytes = resp.bytes().await.map_err(|e| format!("sts read body: {e}"))?;

        if !status.is_success() {
            return match serde_json::from_slice::<StsErrorResponse>(&bytes) {
                Ok(err) => Err(format!(
                    "sts error {}: {}{}",
                    status,
                    err.error,
                    err.error_description.map(|d| format!(" ({d})")).unwrap_or_default()
                )),
                Err(_) => Err(format!(
                    "sts non-success {}: {}",
                    status,
                    String::from_utf8_lossy(&bytes)
                )),
            };
        }

        let body: StsTokenResponse = serde_json::from_slice(&bytes).map_err(|e| format!("sts parse response: {e}"))?;
        let ttl = body.expires_in.unwrap_or(60);
        self.cache_put(key, body.access_token.clone(), ttl);
        Ok(body.access_token)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{body_string_contains, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn http() -> reqwest::Client {
        reqwest::Client::builder().build().unwrap()
    }

    #[tokio::test]
    async fn mint_posts_rfc8693_form_and_returns_access_token() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/oauth2/token"))
            .and(body_string_contains(
                "grant_type=urn%3Aietf%3Aparams%3Aoauth%3Agrant-type%3Atoken-exchange",
            ))
            .and(body_string_contains("audience=mesh.aud"))
            .and(body_string_contains("subject_token=sub.jwt"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "access_token": "minted.jwt",
                "issued_token_type": TOKEN_TYPE_JWT,
                "token_type": "Bearer",
                "expires_in": 120,
            })))
            .mount(&server)
            .await;

        let minter = StsMinter::new(
            http(),
            format!("{}/oauth2/token", server.uri()),
            Arc::new(StaticSubjectProvider::new("sub.jwt")),
        );

        let token = minter
            .mint("mesh.aud", "tenant-a", TokenExchangeMode::Delegation)
            .await
            .unwrap();
        assert_eq!(token, "minted.jwt");
    }

    #[tokio::test]
    async fn mint_forwards_mode_to_sts_form() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/oauth2/token"))
            .and(body_string_contains("mode=ExchangeOnly"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "access_token": "impersonation.jwt",
                "token_type": "Bearer",
                "expires_in": 60,
            })))
            .expect(1)
            .mount(&server)
            .await;

        let minter = StsMinter::new(
            http(),
            format!("{}/oauth2/token", server.uri()),
            Arc::new(StaticSubjectProvider::new("sub.jwt")),
        );

        let token = minter
            .mint("graph.aud", "tenant", TokenExchangeMode::ExchangeOnly)
            .await
            .unwrap();
        assert_eq!(token, "impersonation.jwt");
    }

    #[tokio::test]
    async fn mint_caches_token_for_subsequent_calls() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/oauth2/token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "access_token": "minted.jwt",
                "issued_token_type": TOKEN_TYPE_JWT,
                "token_type": "Bearer",
                "expires_in": 120,
            })))
            .expect(1)
            .mount(&server)
            .await;

        let minter = StsMinter::new(
            http(),
            format!("{}/oauth2/token", server.uri()),
            Arc::new(StaticSubjectProvider::new("sub.jwt")),
        );

        let a = minter
            .mint("aud", "tenant", TokenExchangeMode::Delegation)
            .await
            .unwrap();
        let b = minter
            .mint("aud", "tenant", TokenExchangeMode::Delegation)
            .await
            .unwrap();
        assert_eq!(a, b);
        // wiremock expect(1) above asserts second call hit cache.
    }

    #[tokio::test]
    async fn mint_surfaces_sts_error_body() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/oauth2/token"))
            .respond_with(ResponseTemplate::new(400).set_body_json(serde_json::json!({
                "error": "invalid_target",
                "error_description": "audience not allowed",
            })))
            .mount(&server)
            .await;

        let minter = StsMinter::new(
            http(),
            format!("{}/oauth2/token", server.uri()),
            Arc::new(StaticSubjectProvider::new("sub.jwt")),
        );

        let err = minter
            .mint("forbidden.aud", "tenant", TokenExchangeMode::Delegation)
            .await
            .unwrap_err();
        assert!(err.contains("invalid_target"), "{err}");
        assert!(err.contains("audience not allowed"), "{err}");
    }

    #[tokio::test]
    async fn distinct_tenants_get_distinct_cache_entries() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/oauth2/token"))
            .and(body_string_contains("subject_token=sub-a"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "access_token": "token-for-a",
                "token_type": "Bearer",
                "expires_in": 60,
            })))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/oauth2/token"))
            .and(body_string_contains("subject_token=sub-b"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "access_token": "token-for-b",
                "token_type": "Bearer",
                "expires_in": 60,
            })))
            .mount(&server)
            .await;

        struct TenantSubject;
        #[async_trait]
        impl SubjectProvider for TenantSubject {
            async fn subject_token(&self, tenant: &str) -> Result<String, String> {
                Ok(format!("sub-{tenant}"))
            }
        }

        let minter = StsMinter::new(
            http(),
            format!("{}/oauth2/token", server.uri()),
            Arc::new(TenantSubject),
        );
        let a = minter.mint("aud", "a", TokenExchangeMode::Delegation).await.unwrap();
        let b = minter.mint("aud", "b", TokenExchangeMode::Delegation).await.unwrap();
        assert_eq!(a, "token-for-a");
        assert_eq!(b, "token-for-b");
    }
}
