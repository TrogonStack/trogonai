//! Infisical backend for [`VaultStore`].
//!
//! Enabled with the `infisical` Cargo feature.
//!
//! Token → Infisical mapping:
//! ```text
//! tok_anthropic_prod_a1b2c3  →  environment: prod, secretName: anthropic_a1b2c3
//! ```

use reqwest::Client;
use serde_json::Value;

use crate::token::ApiKeyToken;
use crate::vault::VaultStore;

// ── Public types ──────────────────────────────────────────────────────────────

/// How the client authenticates with Infisical.
pub enum InfisicalAuth {
    /// Service token (legacy). Sent as `Authorization: Bearer {token}`.
    ServiceToken(String),
}

/// Configuration for [`InfisicalVaultStore`].
pub struct InfisicalConfig {
    /// Base URL, e.g. `https://app.infisical.com` or a self-hosted address.
    pub base_url: String,
    /// Infisical workspace (project) ID.
    pub project_id: String,
    /// Secret path within each environment. Defaults to `/`.
    pub secret_path: String,
    /// Authentication method.
    pub auth: InfisicalAuth,
}

impl InfisicalConfig {
    pub fn new(
        base_url: impl Into<String>,
        project_id: impl Into<String>,
        auth: InfisicalAuth,
    ) -> Self {
        Self {
            base_url: base_url.into(),
            project_id: project_id.into(),
            secret_path: "/".to_string(),
            auth,
        }
    }

    pub fn with_secret_path(mut self, path: impl Into<String>) -> Self {
        self.secret_path = path.into();
        self
    }

    /// Build an [`InfisicalConfig`] from environment variables.
    ///
    /// | Variable                  | Description                        |
    /// |---------------------------|------------------------------------|
    /// | `INFISICAL_URL`           | Base URL (e.g. `https://app.infisical.com`) |
    /// | `INFISICAL_PROJECT_ID`    | Workspace / project ID             |
    /// | `INFISICAL_SERVICE_TOKEN` | Service token for authentication   |
    ///
    /// Returns an error message listing any missing variables.
    pub fn from_env() -> Result<Self, String> {
        let mut missing = Vec::new();

        let base_url = std::env::var("INFISICAL_URL").unwrap_or_else(|_| {
            missing.push("INFISICAL_URL");
            String::new()
        });
        let project_id = std::env::var("INFISICAL_PROJECT_ID").unwrap_or_else(|_| {
            missing.push("INFISICAL_PROJECT_ID");
            String::new()
        });
        let token = std::env::var("INFISICAL_SERVICE_TOKEN").unwrap_or_else(|_| {
            missing.push("INFISICAL_SERVICE_TOKEN");
            String::new()
        });

        if !missing.is_empty() {
            return Err(format!("missing env vars: {}", missing.join(", ")));
        }

        Ok(Self::new(base_url, project_id, InfisicalAuth::ServiceToken(token)))
    }
}

/// Errors produced by [`InfisicalVaultStore`].
#[derive(Debug)]
pub enum InfisicalError {
    /// An HTTP transport error.
    Http(reqwest::Error),
    /// Infisical returned a non-2xx status code.
    Api { status: u16, message: String },
    /// Could not deserialize an Infisical response.
    Deserialize(String),
}

impl std::fmt::Display for InfisicalError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Http(e) => write!(f, "HTTP error: {e}"),
            Self::Api { status, message } => {
                write!(f, "Infisical API error ({status}): {message}")
            }
            Self::Deserialize(msg) => write!(f, "deserialization error: {msg}"),
        }
    }
}

impl std::error::Error for InfisicalError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Http(e) => Some(e),
            _ => None,
        }
    }
}

// ── Store ─────────────────────────────────────────────────────────────────────

/// [`VaultStore`] backend backed by an Infisical instance.
///
/// Tokens are mapped to Infisical secrets as:
/// `tok_{provider}_{env}_{id}` → environment `{env}`, secret name `{provider}_{id}`
pub struct InfisicalVaultStore {
    client:      Client,
    base_url:    String,
    project_id:  String,
    secret_path: String,
    token:       String,
}

impl InfisicalVaultStore {
    pub fn new(config: InfisicalConfig) -> Self {
        let token = match config.auth {
            InfisicalAuth::ServiceToken(t) => t,
        };
        Self {
            client: Client::new(),
            base_url: config.base_url,
            project_id: config.project_id,
            secret_path: config.secret_path,
            token,
        }
    }

    fn secret_name(&self, token: &ApiKeyToken) -> String {
        format!("{}_{}", token.provider_str(), token.id_str())
    }

    fn secret_url(&self, name: &str) -> String {
        format!("{}/api/v3/secrets/raw/{}", self.base_url, name)
    }

    fn bearer(&self) -> String {
        format!("Bearer {}", self.token)
    }

    async fn patch_secret(
        &self,
        name: &str,
        environment: &str,
        plaintext: &str,
    ) -> Result<(), InfisicalError> {
        let resp = self
            .client
            .patch(self.secret_url(name))
            .header("Authorization", self.bearer())
            .json(&serde_json::json!({
                "workspaceId": self.project_id,
                "environment": environment,
                "secretPath": self.secret_path,
                "secretValue": plaintext,
            }))
            .send()
            .await
            .map_err(InfisicalError::Http)?;

        let status = resp.status();
        if status.is_success() {
            Ok(())
        } else {
            Err(InfisicalError::Api {
                status:  status.as_u16(),
                message: parse_error(resp).await,
            })
        }
    }
}

// ── VaultStore impl ───────────────────────────────────────────────────────────

impl VaultStore for InfisicalVaultStore {
    type Error = InfisicalError;

    async fn store(&self, token: &ApiKeyToken, plaintext: &str) -> Result<(), Self::Error> {
        let name = self.secret_name(token);
        let env  = token.env_str().to_string();

        let resp = self
            .client
            .post(self.secret_url(&name))
            .header("Authorization", self.bearer())
            .json(&serde_json::json!({
                "workspaceId": self.project_id,
                "environment": env,
                "secretPath": self.secret_path,
                "secretValue": plaintext,
                "type": "shared",
            }))
            .send()
            .await
            .map_err(InfisicalError::Http)?;

        let status = resp.status();
        if status.is_success() {
            return Ok(());
        }

        // 409 Conflict = secret already exists → update in place.
        if status.as_u16() == 409 {
            return self.patch_secret(&name, &env, plaintext).await;
        }

        Err(InfisicalError::Api {
            status:  status.as_u16(),
            message: parse_error(resp).await,
        })
    }

    async fn resolve(&self, token: &ApiKeyToken) -> Result<Option<String>, Self::Error> {
        let name = self.secret_name(token);
        let env  = token.env_str().to_string();

        let resp = self
            .client
            .get(self.secret_url(&name))
            .header("Authorization", self.bearer())
            .query(&[
                ("workspaceId", self.project_id.as_str()),
                ("environment", env.as_str()),
                ("secretPath", self.secret_path.as_str()),
            ])
            .send()
            .await
            .map_err(InfisicalError::Http)?;

        let status = resp.status();
        if status.as_u16() == 404 {
            return Ok(None);
        }

        if status.is_success() {
            let json: Value = resp.json().await.map_err(InfisicalError::Http)?;
            let value = json
                .pointer("/secret/secretValue")
                .and_then(|v| v.as_str())
                .map(String::from)
                .ok_or_else(|| {
                    InfisicalError::Deserialize(
                        "missing .secret.secretValue in Infisical response".to_string(),
                    )
                })?;
            return Ok(Some(value));
        }

        Err(InfisicalError::Api {
            status:  status.as_u16(),
            message: parse_error(resp).await,
        })
    }

    async fn revoke(&self, token: &ApiKeyToken) -> Result<(), Self::Error> {
        let name = self.secret_name(token);
        let env  = token.env_str().to_string();

        let resp = self
            .client
            .delete(self.secret_url(&name))
            .header("Authorization", self.bearer())
            .query(&[
                ("workspaceId", self.project_id.as_str()),
                ("environment", env.as_str()),
                ("secretPath", self.secret_path.as_str()),
            ])
            .send()
            .await
            .map_err(InfisicalError::Http)?;

        let status = resp.status();
        if status.is_success() || status.as_u16() == 404 {
            return Ok(());
        }

        Err(InfisicalError::Api {
            status:  status.as_u16(),
            message: parse_error(resp).await,
        })
    }

    // rotate() uses the default impl (delegates to store), which handles
    // create-vs-update transparently via the POST → 409 → PATCH path.
}

async fn parse_error(resp: reqwest::Response) -> String {
    resp.json::<Value>()
        .await
        .ok()
        .and_then(|v| v.get("message")?.as_str().map(String::from))
        .unwrap_or_else(|| "unknown error".to_string())
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use httpmock::prelude::*;
    use httpmock::Method::PATCH;

    fn tok(s: &str) -> ApiKeyToken {
        ApiKeyToken::new(s).unwrap()
    }

    fn store(server: &MockServer) -> InfisicalVaultStore {
        InfisicalVaultStore::new(InfisicalConfig::new(
            format!("http://{}", server.address()),
            "proj-abc123",
            InfisicalAuth::ServiceToken("st.test-token".to_string()),
        ))
    }

    // ── Mapping helpers ───────────────────────────────────────────────────────

    #[test]
    fn secret_name_strips_prefix_and_env() {
        let vault = InfisicalVaultStore::new(InfisicalConfig::new(
            "http://localhost",
            "p1",
            InfisicalAuth::ServiceToken("t".to_string()),
        ));
        let token = tok("tok_anthropic_prod_a1b2c3");
        assert_eq!(vault.secret_name(&token), "anthropic_a1b2c3");
    }

    #[test]
    fn secret_url_format() {
        let vault = InfisicalVaultStore::new(InfisicalConfig::new(
            "https://infisical.example.com",
            "p1",
            InfisicalAuth::ServiceToken("t".to_string()),
        ));
        let token = tok("tok_openai_staging_xyz789");
        assert_eq!(
            vault.secret_url(&vault.secret_name(&token)),
            "https://infisical.example.com/api/v3/secrets/raw/openai_xyz789"
        );
    }

    // ── store() ───────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn store_posts_to_correct_path() {
        let server = MockServer::start();
        let m = server.mock(|when, then| {
            when.method(POST)
                .path("/api/v3/secrets/raw/anthropic_a1b2c3")
                .header("Authorization", "Bearer st.test-token")
                .json_body_partial(r#"{"secretValue":"sk-ant-key"}"#);
            then.status(200)
                .json_body(serde_json::json!({"secret": {"secretKey": "anthropic_a1b2c3"}}));
        });

        let vault = store(&server);
        vault.store(&tok("tok_anthropic_prod_a1b2c3"), "sk-ant-key").await.unwrap();
        m.assert();
    }

    #[tokio::test]
    async fn store_patches_on_409_conflict() {
        let server = MockServer::start();
        let post_m = server.mock(|when, then| {
            when.method(POST).path("/api/v3/secrets/raw/anthropic_a1b2c3");
            then.status(409)
                .json_body(serde_json::json!({"message": "Secret already exists"}));
        });
        let patch_m = server.mock(|when, then| {
            when.method(PATCH)
                .path("/api/v3/secrets/raw/anthropic_a1b2c3")
                .json_body_partial(r#"{"secretValue":"sk-ant-key"}"#);
            then.status(200)
                .json_body(serde_json::json!({"secret": {}}));
        });

        let vault = store(&server);
        vault.store(&tok("tok_anthropic_prod_a1b2c3"), "sk-ant-key").await.unwrap();
        post_m.assert();
        patch_m.assert();
    }

    #[tokio::test]
    async fn store_propagates_api_errors() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(POST).path("/api/v3/secrets/raw/anthropic_a1b2c3");
            then.status(403)
                .json_body(serde_json::json!({"message": "Unauthorized"}));
        });

        let err = store(&server)
            .store(&tok("tok_anthropic_prod_a1b2c3"), "sk-ant-key")
            .await
            .unwrap_err();
        assert!(matches!(err, InfisicalError::Api { status: 403, .. }));
    }

    // ── resolve() ─────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn resolve_returns_secret_value() {
        let server = MockServer::start();
        let m = server.mock(|when, then| {
            when.method(GET)
                .path("/api/v3/secrets/raw/anthropic_a1b2c3")
                .query_param("environment", "prod");
            then.status(200).json_body(serde_json::json!({
                "secret": { "secretKey": "anthropic_a1b2c3", "secretValue": "sk-ant-real" }
            }));
        });

        let result = store(&server)
            .resolve(&tok("tok_anthropic_prod_a1b2c3"))
            .await
            .unwrap();
        assert_eq!(result, Some("sk-ant-real".to_string()));
        m.assert();
    }

    #[tokio::test]
    async fn resolve_returns_none_on_404() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(GET).path("/api/v3/secrets/raw/anthropic_a1b2c3");
            then.status(404)
                .json_body(serde_json::json!({"message": "Secret not found"}));
        });

        let result = store(&server)
            .resolve(&tok("tok_anthropic_prod_a1b2c3"))
            .await
            .unwrap();
        assert_eq!(result, None);
    }

    #[tokio::test]
    async fn resolve_returns_deserialize_error_when_field_missing() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(GET).path("/api/v3/secrets/raw/anthropic_a1b2c3");
            then.status(200)
                .json_body(serde_json::json!({"secret": {}}));
        });

        let err = store(&server)
            .resolve(&tok("tok_anthropic_prod_a1b2c3"))
            .await
            .unwrap_err();
        assert!(matches!(err, InfisicalError::Deserialize(_)));
    }

    #[tokio::test]
    async fn resolve_propagates_api_errors() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(GET).path("/api/v3/secrets/raw/anthropic_a1b2c3");
            then.status(500)
                .json_body(serde_json::json!({"message": "Internal server error"}));
        });

        let err = store(&server)
            .resolve(&tok("tok_anthropic_prod_a1b2c3"))
            .await
            .unwrap_err();
        assert!(matches!(err, InfisicalError::Api { status: 500, .. }));
    }

    // ── revoke() ──────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn revoke_sends_delete() {
        let server = MockServer::start();
        let m = server.mock(|when, then| {
            when.method(DELETE)
                .path("/api/v3/secrets/raw/anthropic_a1b2c3")
                .query_param("environment", "prod");
            then.status(200).json_body(serde_json::json!({"secret": {}}));
        });

        store(&server)
            .revoke(&tok("tok_anthropic_prod_a1b2c3"))
            .await
            .unwrap();
        m.assert();
    }

    #[tokio::test]
    async fn revoke_is_idempotent_on_404() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(DELETE).path("/api/v3/secrets/raw/anthropic_a1b2c3");
            then.status(404)
                .json_body(serde_json::json!({"message": "Secret not found"}));
        });

        let result = store(&server)
            .revoke(&tok("tok_anthropic_prod_a1b2c3"))
            .await;
        assert!(result.is_ok(), "404 on revoke must be treated as success");
    }

    #[tokio::test]
    async fn revoke_propagates_api_errors() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(DELETE).path("/api/v3/secrets/raw/anthropic_a1b2c3");
            then.status(403)
                .json_body(serde_json::json!({"message": "Unauthorized"}));
        });

        let err = store(&server)
            .revoke(&tok("tok_anthropic_prod_a1b2c3"))
            .await
            .unwrap_err();
        assert!(matches!(err, InfisicalError::Api { status: 403, .. }));
    }

    // ── rotate() ──────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn rotate_patches_existing_secret() {
        let server = MockServer::start();
        // POST returns 409 → PATCH is called.
        server.mock(|when, then| {
            when.method(POST).path("/api/v3/secrets/raw/anthropic_a1b2c3");
            then.status(409)
                .json_body(serde_json::json!({"message": "Secret already exists"}));
        });
        let patch_m = server.mock(|when, then| {
            when.method(PATCH)
                .path("/api/v3/secrets/raw/anthropic_a1b2c3")
                .json_body_partial(r#"{"secretValue":"sk-ant-v2"}"#);
            then.status(200).json_body(serde_json::json!({"secret": {}}));
        });

        store(&server)
            .rotate(&tok("tok_anthropic_prod_a1b2c3"), "sk-ant-v2")
            .await
            .unwrap();
        patch_m.assert();
    }

    #[tokio::test]
    async fn rotate_creates_when_secret_does_not_exist() {
        let server = MockServer::start();
        let post_m = server.mock(|when, then| {
            when.method(POST)
                .path("/api/v3/secrets/raw/anthropic_a1b2c3")
                .json_body_partial(r#"{"secretValue":"sk-ant-new"}"#);
            then.status(200).json_body(serde_json::json!({"secret": {}}));
        });

        store(&server)
            .rotate(&tok("tok_anthropic_prod_a1b2c3"), "sk-ant-new")
            .await
            .unwrap();
        post_m.assert();
    }

    // ── environment routing ───────────────────────────────────────────────────

    #[tokio::test]
    async fn store_uses_token_env_as_infisical_environment() {
        let server = MockServer::start();
        let m = server.mock(|when, then| {
            when.method(POST)
                .path("/api/v3/secrets/raw/openai_xyz789")
                .json_body_partial(r#"{"environment":"staging"}"#);
            then.status(200).json_body(serde_json::json!({"secret": {}}));
        });

        store(&server)
            .store(&tok("tok_openai_staging_xyz789"), "sk-openai-key")
            .await
            .unwrap();
        m.assert();
    }

    // ── parse_error: non-JSON body ────────────────────────────────────────────

    #[tokio::test]
    async fn api_error_with_non_json_body_uses_fallback_message() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(GET).path("/api/v3/secrets/raw/anthropic_a1b2c3");
            then.status(502)
                .header("content-type", "text/plain")
                .body("Bad Gateway");
        });

        let err = store(&server)
            .resolve(&tok("tok_anthropic_prod_a1b2c3"))
            .await
            .unwrap_err();
        match err {
            InfisicalError::Api { status, message } => {
                assert_eq!(status, 502);
                assert_eq!(message, "unknown error");
            }
            other => panic!("expected Api error, got: {other}"),
        }
    }
}
