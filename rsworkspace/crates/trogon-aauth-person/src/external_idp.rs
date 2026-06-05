//! External IdP second-leg token exchange.
//!
//! After [`WifExchangeService`](crate::wif_exchange::WifExchangeService) has
//! resolved the inbound subject token, the gateway sometimes needs an
//! upstream access token issued by an external IdP (Microsoft Entra, Okta,
//! a generic OIDC server). This module performs the second-leg call using
//! RFC 7523 / RFC 8693 `urn:ietf:params:oauth:grant-type:jwt-bearer` so the
//! caller can inject the resulting `Authorization: Bearer …` into the
//! outgoing MCP request.

use std::time::Duration;

use serde::{Deserialize, Serialize};

pub const GRANT_TYPE_JWT_BEARER: &str = "urn:ietf:params:oauth:grant-type:jwt-bearer";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExternalIdpConfig {
    pub token_url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_secret: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub scopes: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audience: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct UpstreamAccessToken {
    pub access_token: String,
    pub token_type: String,
    pub expires_in: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum ExternalIdpError {
    #[error("http error: {0}")]
    Http(String),
    #[error("upstream returned {status}: {body}")]
    Upstream { status: u16, body: String },
    #[error("decode upstream response: {0}")]
    Decode(String),
}

pub struct ExternalIdpClient {
    http: reqwest::Client,
}

impl Default for ExternalIdpClient {
    fn default() -> Self {
        Self::new(Duration::from_secs(10))
    }
}

impl ExternalIdpClient {
    pub fn new(timeout: Duration) -> Self {
        let http = reqwest::Client::builder()
            .timeout(timeout)
            .build()
            .expect("reqwest client");
        Self { http }
    }

    pub async fn exchange_jwt_bearer(
        &self,
        cfg: &ExternalIdpConfig,
        assertion: &str,
    ) -> Result<UpstreamAccessToken, ExternalIdpError> {
        let mut form: Vec<(&str, String)> = vec![
            ("grant_type", GRANT_TYPE_JWT_BEARER.to_string()),
            ("assertion", assertion.to_string()),
        ];
        if !cfg.scopes.is_empty() {
            form.push(("scope", cfg.scopes.join(" ")));
        }
        if let Some(audience) = &cfg.audience {
            form.push(("audience", audience.clone()));
        }
        if let Some(client_id) = &cfg.client_id {
            form.push(("client_id", client_id.clone()));
        }
        if let Some(client_secret) = &cfg.client_secret {
            form.push(("client_secret", client_secret.clone()));
        }

        let response = self
            .http
            .post(&cfg.token_url)
            .form(&form)
            .send()
            .await
            .map_err(|e| ExternalIdpError::Http(e.to_string()))?;
        let status = response.status();
        let body = response
            .text()
            .await
            .map_err(|e| ExternalIdpError::Http(e.to_string()))?;
        if !status.is_success() {
            return Err(ExternalIdpError::Upstream {
                status: status.as_u16(),
                body,
            });
        }
        serde_json::from_str::<UpstreamAccessToken>(&body).map_err(|e| ExternalIdpError::Decode(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{body_string_contains, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn jwt_bearer_exchange_returns_upstream_access_token() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/oauth2/v2.0/token"))
            .and(body_string_contains("grant_type=urn"))
            .and(body_string_contains("assertion=inbound.jwt"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "access_token": "entra.access.token",
                "token_type": "Bearer",
                "expires_in": 3600,
                "scope": "https://graph.microsoft.com/.default",
            })))
            .expect(1)
            .mount(&server)
            .await;

        let cfg = ExternalIdpConfig {
            token_url: format!("{}/oauth2/v2.0/token", server.uri()),
            client_id: Some("client".into()),
            client_secret: Some("secret".into()),
            scopes: vec!["https://graph.microsoft.com/.default".into()],
            audience: None,
        };
        let client = ExternalIdpClient::default();
        let token = client
            .exchange_jwt_bearer(&cfg, "inbound.jwt")
            .await
            .expect("exchange");
        assert_eq!(token.access_token, "entra.access.token");
        assert_eq!(token.expires_in, 3600);
    }

    #[tokio::test]
    async fn jwt_bearer_exchange_surfaces_upstream_error() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/oauth2/v2.0/token"))
            .respond_with(ResponseTemplate::new(400).set_body_string(
                r#"{"error":"invalid_grant","error_description":"AADSTS50013"}"#,
            ))
            .mount(&server)
            .await;
        let cfg = ExternalIdpConfig {
            token_url: format!("{}/oauth2/v2.0/token", server.uri()),
            client_id: None,
            client_secret: None,
            scopes: vec![],
            audience: None,
        };
        let client = ExternalIdpClient::default();
        let err = client
            .exchange_jwt_bearer(&cfg, "bad.jwt")
            .await
            .expect_err("upstream error");
        assert!(matches!(err, ExternalIdpError::Upstream { status: 400, .. }));
    }
}
