//! HashiCorp Vault (and OpenBao) KV v2 backend for [`VaultStore`].
//!
//! Enabled with the `hashicorp-vault` Cargo feature.
//!
//! Token → Vault path mapping:
//! ```text
//! tok_anthropic_prod_a1b2c3  →  {mount}/data/anthropic/prod/a1b2c3
//! ```

use std::future::Future;
use std::sync::RwLock;

use reqwest::Client;
use serde_json::Value;

use crate::token::ApiKeyToken;
use crate::vault::VaultStore;

// ── Public types ──────────────────────────────────────────────────────────────

/// How the client authenticates with Vault / OpenBao.
pub enum VaultAuth {
    /// Static Vault token (e.g. from `VAULT_TOKEN`). No re-authentication is attempted.
    Token(String),
    /// AppRole authentication. A new token is obtained via `/v1/auth/approle/login`.
    AppRole { role_id: String, secret_id: String },
    /// Kubernetes service-account JWT authentication.
    Kubernetes {
        role: String,
        /// Path to the JWT file. Defaults to the Kubernetes SA token path when `None`.
        jwt_path: Option<String>,
    },
}

/// Configuration for [`HashicorpVaultStore`].
pub struct HashicorpVaultConfig {
    pub vault_addr: String,
    pub mount: String,
    pub auth: VaultAuth,
    pub tls_skip_verify: bool,
}

impl HashicorpVaultConfig {
    pub fn new(
        vault_addr: impl Into<String>,
        mount: impl Into<String>,
        auth: VaultAuth,
    ) -> Self {
        Self {
            vault_addr: vault_addr.into(),
            mount: mount.into(),
            auth,
            tls_skip_verify: false,
        }
    }

    /// Accept self-signed TLS certificates. **Only for development.**
    pub fn with_tls_skip_verify(mut self) -> Self {
        self.tls_skip_verify = true;
        self
    }
}

/// Errors produced by [`HashicorpVaultStore`].
#[derive(Debug)]
pub enum HashicorpVaultError {
    /// An HTTP transport error.
    Http(reqwest::Error),
    /// Vault returned a non-2xx status code.
    Api { status: u16, errors: Vec<String> },
    /// Authentication failed or the response is missing a client token.
    Auth(String),
    /// Could not deserialize a Vault response.
    Deserialize(String),
    /// I/O error (e.g. reading the Kubernetes SA JWT file).
    Io(String),
}

impl std::fmt::Display for HashicorpVaultError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Http(e) => write!(f, "HTTP error: {e}"),
            Self::Api { status, errors } => {
                write!(f, "Vault API error ({status}): {}", errors.join(", "))
            }
            Self::Auth(msg) => write!(f, "Vault auth error: {msg}"),
            Self::Deserialize(msg) => write!(f, "deserialization error: {msg}"),
            Self::Io(msg) => write!(f, "I/O error: {msg}"),
        }
    }
}

impl std::error::Error for HashicorpVaultError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Http(e) => Some(e),
            _ => None,
        }
    }
}

// ── Store ─────────────────────────────────────────────────────────────────────

/// [`VaultStore`] backend backed by a HashiCorp Vault (or OpenBao) KV v2 mount.
///
/// Tokens are mapped to Vault paths as:
/// `tok_{provider}_{env}_{id}` → `{mount}/data/{provider}/{env}/{id}`
pub struct HashicorpVaultStore {
    client: Client,
    vault_addr: String,
    mount: String,
    /// Current Vault token. Stored in a `RwLock` so re-authentication can update
    /// it without requiring `&mut self`. Always cloned before any `.await`.
    token: RwLock<String>,
    /// Kept so that `with_reauth` can obtain a fresh token on 403 responses.
    auth: VaultAuth,
}

impl HashicorpVaultStore {
    /// Create a new store and authenticate with Vault.
    ///
    /// For [`VaultAuth::Token`] no network call is made; for other methods an
    /// initial login request is performed.
    pub async fn new(config: HashicorpVaultConfig) -> Result<Self, HashicorpVaultError> {
        let client = if config.tls_skip_verify {
            Client::builder()
                .danger_accept_invalid_certs(true)
                .build()
                .map_err(HashicorpVaultError::Http)?
        } else {
            Client::new()
        };

        let initial_token = authenticate(&client, &config.vault_addr, &config.auth).await?;

        Ok(Self {
            client,
            vault_addr: config.vault_addr,
            mount: config.mount,
            token: RwLock::new(initial_token),
            auth: config.auth,
        })
    }

    // ── URL helpers ───────────────────────────────────────────────────────────

    fn data_url(&self, token: &ApiKeyToken) -> String {
        format!(
            "{}/v1/{}/data/{}/{}/{}",
            self.vault_addr,
            self.mount,
            token.provider_str(),
            token.env_str(),
            token.id_str(),
        )
    }

    fn metadata_url(&self, token: &ApiKeyToken) -> String {
        format!(
            "{}/v1/{}/metadata/{}/{}/{}",
            self.vault_addr,
            self.mount,
            token.provider_str(),
            token.env_str(),
            token.id_str(),
        )
    }

    // ── Vault token management ────────────────────────────────────────────────

    /// Clone the current Vault token, dropping the read lock immediately.
    fn current_token(&self) -> String {
        self.token.read().unwrap().clone()
    }

    fn set_token(&self, t: String) {
        *self.token.write().unwrap() = t;
    }

    // ── Re-authentication ─────────────────────────────────────────────────────

    async fn reauthenticate(&self) -> Result<(), HashicorpVaultError> {
        let new_token = authenticate(&self.client, &self.vault_addr, &self.auth).await?;
        self.set_token(new_token);
        Ok(())
    }

    /// Execute `f` with the current Vault token.
    ///
    /// If `f` returns a 403 error **and** the configured auth method is not a
    /// static token, re-authenticates once and retries. For static-token auth a
    /// 403 is returned immediately (no new credentials to obtain).
    async fn with_reauth<F, Fut, T>(&self, f: F) -> Result<T, HashicorpVaultError>
    where
        F: Fn(String) -> Fut + Send,
        Fut: Future<Output = Result<T, HashicorpVaultError>> + Send,
        T: Send,
    {
        let first = f(self.current_token()).await;

        let should_retry = match &first {
            Err(HashicorpVaultError::Api { status, .. }) if *status == 403 => {
                !matches!(self.auth, VaultAuth::Token(_))
            }
            _ => false,
        };

        if should_retry {
            self.reauthenticate().await?;
            f(self.current_token()).await
        } else {
            first
        }
    }
}

// ── Free-standing auth helpers ────────────────────────────────────────────────

async fn authenticate(
    client: &Client,
    vault_addr: &str,
    auth: &VaultAuth,
) -> Result<String, HashicorpVaultError> {
    match auth {
        VaultAuth::Token(t) => Ok(t.clone()),
        VaultAuth::AppRole { role_id, secret_id } => {
            approle_login(client, vault_addr, role_id, secret_id).await
        }
        VaultAuth::Kubernetes { role, jwt_path } => {
            kubernetes_login(client, vault_addr, role, jwt_path.as_deref()).await
        }
    }
}

async fn approle_login(
    client: &Client,
    vault_addr: &str,
    role_id: &str,
    secret_id: &str,
) -> Result<String, HashicorpVaultError> {
    let url = format!("{vault_addr}/v1/auth/approle/login");
    let body = serde_json::json!({"role_id": role_id, "secret_id": secret_id});
    let resp = client
        .post(&url)
        .json(&body)
        .send()
        .await
        .map_err(HashicorpVaultError::Http)?;
    extract_client_token(resp, "approle").await
}

async fn kubernetes_login(
    client: &Client,
    vault_addr: &str,
    role: &str,
    jwt_path: Option<&str>,
) -> Result<String, HashicorpVaultError> {
    let path = jwt_path.unwrap_or("/var/run/secrets/kubernetes.io/serviceaccount/token");
    let jwt = std::fs::read_to_string(path)
        .map_err(|e| HashicorpVaultError::Io(e.to_string()))?;
    let jwt = jwt.trim().to_string();

    let url = format!("{vault_addr}/v1/auth/kubernetes/login");
    let body = serde_json::json!({"role": role, "jwt": jwt});
    let resp = client
        .post(&url)
        .json(&body)
        .send()
        .await
        .map_err(HashicorpVaultError::Http)?;
    extract_client_token(resp, "kubernetes").await
}

async fn extract_client_token(
    resp: reqwest::Response,
    method: &str,
) -> Result<String, HashicorpVaultError> {
    if resp.status().is_success() {
        let json: Value = resp.json().await.map_err(HashicorpVaultError::Http)?;
        json.pointer("/auth/client_token")
            .and_then(|v| v.as_str())
            .map(String::from)
            .ok_or_else(|| {
                HashicorpVaultError::Auth(format!(
                    "missing client_token in {method} login response"
                ))
            })
    } else {
        let status = resp.status().as_u16();
        let errors = parse_vault_errors(resp).await;
        Err(HashicorpVaultError::Api { status, errors })
    }
}

async fn parse_vault_errors(resp: reqwest::Response) -> Vec<String> {
    resp.json::<Value>()
        .await
        .ok()
        .and_then(|v| {
            v.get("errors")?.as_array().map(|arr| {
                arr.iter()
                    .filter_map(|e| e.as_str().map(String::from))
                    .collect()
            })
        })
        .unwrap_or_default()
}

// ── VaultStore impl ───────────────────────────────────────────────────────────

impl VaultStore for HashicorpVaultStore {
    type Error = HashicorpVaultError;

    async fn store(&self, token: &ApiKeyToken, plaintext: &str) -> Result<(), Self::Error> {
        let url = self.data_url(token);
        let client = self.client.clone();
        let body = serde_json::json!({"data": {"api_key": plaintext}});

        self.with_reauth(move |vault_token| {
            let url = url.clone();
            let client = client.clone();
            let body = body.clone();
            async move {
                let resp = client
                    .put(&url)
                    .header("X-Vault-Token", vault_token)
                    .json(&body)
                    .send()
                    .await
                    .map_err(HashicorpVaultError::Http)?;

                if resp.status().is_success() {
                    Ok(())
                } else {
                    let status = resp.status().as_u16();
                    let errors = parse_vault_errors(resp).await;
                    Err(HashicorpVaultError::Api { status, errors })
                }
            }
        })
        .await
    }

    async fn resolve(&self, token: &ApiKeyToken) -> Result<Option<String>, Self::Error> {
        let url = self.data_url(token);
        let client = self.client.clone();

        self.with_reauth(move |vault_token| {
            let url = url.clone();
            let client = client.clone();
            async move {
                let resp = client
                    .get(&url)
                    .header("X-Vault-Token", vault_token)
                    .send()
                    .await
                    .map_err(HashicorpVaultError::Http)?;

                let status = resp.status();
                if status.as_u16() == 404 {
                    return Ok(None);
                }

                if status.is_success() {
                    let json: Value = resp.json().await.map_err(HashicorpVaultError::Http)?;
                    let key = json
                        .pointer("/data/data/api_key")
                        .and_then(|v| v.as_str())
                        .map(String::from)
                        .ok_or_else(|| {
                            HashicorpVaultError::Deserialize(
                                "missing .data.data.api_key in Vault response".to_string(),
                            )
                        })?;
                    Ok(Some(key))
                } else {
                    let status_u16 = status.as_u16();
                    let errors = parse_vault_errors(resp).await;
                    Err(HashicorpVaultError::Api {
                        status: status_u16,
                        errors,
                    })
                }
            }
        })
        .await
    }

    async fn revoke(&self, token: &ApiKeyToken) -> Result<(), Self::Error> {
        let url = self.metadata_url(token);
        let client = self.client.clone();

        self.with_reauth(move |vault_token| {
            let url = url.clone();
            let client = client.clone();
            async move {
                let resp = client
                    .delete(&url)
                    .header("X-Vault-Token", vault_token)
                    .send()
                    .await
                    .map_err(HashicorpVaultError::Http)?;

                let status = resp.status();
                if status.is_success() || status.as_u16() == 404 {
                    Ok(())
                } else {
                    let status_u16 = status.as_u16();
                    let errors = parse_vault_errors(resp).await;
                    Err(HashicorpVaultError::Api {
                        status: status_u16,
                        errors,
                    })
                }
            }
        })
        .await
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use httpmock::prelude::*;

    fn tok(s: &str) -> ApiKeyToken {
        ApiKeyToken::new(s).unwrap()
    }

    /// Build a Token-auth store pointing at the given mock server.
    async fn token_store(server: &MockServer) -> HashicorpVaultStore {
        let vault_addr = format!("http://{}", server.address());
        let config = HashicorpVaultConfig::new(
            &vault_addr,
            "ai-keys",
            VaultAuth::Token("test-token".to_string()),
        );
        HashicorpVaultStore::new(config).await.unwrap()
    }

    // ── Pure (no HTTP) ────────────────────────────────────────────────────────

    #[tokio::test]
    async fn data_url_format() {
        let config = HashicorpVaultConfig::new(
            "https://vault.example.com:8200",
            "ai-keys",
            VaultAuth::Token("t".to_string()),
        );
        let store = HashicorpVaultStore::new(config).await.unwrap();
        let token = tok("tok_anthropic_prod_a1b2c3");
        assert_eq!(
            store.data_url(&token),
            "https://vault.example.com:8200/v1/ai-keys/data/anthropic/prod/a1b2c3"
        );
    }

    #[tokio::test]
    async fn metadata_url_format() {
        let config = HashicorpVaultConfig::new(
            "https://vault.example.com:8200",
            "ai-keys",
            VaultAuth::Token("t".to_string()),
        );
        let store = HashicorpVaultStore::new(config).await.unwrap();
        let token = tok("tok_anthropic_prod_a1b2c3");
        assert_eq!(
            store.metadata_url(&token),
            "https://vault.example.com:8200/v1/ai-keys/metadata/anthropic/prod/a1b2c3"
        );
    }

    // ── httpmock tests ────────────────────────────────────────────────────────

    #[tokio::test]
    async fn store_sends_correct_body() {
        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(PUT)
                .path("/v1/ai-keys/data/anthropic/prod/a1b2c3")
                .json_body(serde_json::json!({"data": {"api_key": "sk-ant-realkey"}}));
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"request_id":"req1"}"#);
        });

        let store = token_store(&server).await;
        let token = tok("tok_anthropic_prod_a1b2c3");
        store.store(&token, "sk-ant-realkey").await.unwrap();
        mock.assert();
    }

    #[tokio::test]
    async fn resolve_returns_api_key() {
        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(GET)
                .path("/v1/ai-keys/data/anthropic/prod/a1b2c3");
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"data":{"data":{"api_key":"sk-ant-realkey"},"metadata":{}}}"#);
        });

        let store = token_store(&server).await;
        let token = tok("tok_anthropic_prod_a1b2c3");
        let result = store.resolve(&token).await.unwrap();
        assert_eq!(result, Some("sk-ant-realkey".to_string()));
        mock.assert();
    }

    #[tokio::test]
    async fn resolve_returns_none_for_404() {
        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(GET)
                .path("/v1/ai-keys/data/anthropic/prod/a1b2c3");
            then.status(404)
                .header("content-type", "application/json")
                .body(r#"{"errors":[]}"#);
        });

        let store = token_store(&server).await;
        let token = tok("tok_anthropic_prod_a1b2c3");
        let result = store.resolve(&token).await.unwrap();
        assert_eq!(result, None);
        mock.assert();
    }

    #[tokio::test]
    async fn revoke_deletes_metadata_path() {
        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(DELETE)
                .path("/v1/ai-keys/metadata/anthropic/prod/a1b2c3");
            then.status(204);
        });

        let store = token_store(&server).await;
        let token = tok("tok_anthropic_prod_a1b2c3");
        store.revoke(&token).await.unwrap();
        mock.assert();
    }

    #[tokio::test]
    async fn api_error_propagated() {
        let server = MockServer::start();
        let _mock = server.mock(|when, then| {
            when.method(GET)
                .path("/v1/ai-keys/data/anthropic/prod/a1b2c3");
            then.status(403)
                .header("content-type", "application/json")
                .body(r#"{"errors":["permission denied"]}"#);
        });

        // Token auth — no re-authentication, so 403 is returned immediately.
        let store = token_store(&server).await;
        let token = tok("tok_anthropic_prod_a1b2c3");
        let result = store.resolve(&token).await;
        assert!(matches!(result, Err(HashicorpVaultError::Api { status: 403, .. })));
    }

    #[tokio::test]
    async fn approle_login_on_new() {
        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(POST)
                .path("/v1/auth/approle/login")
                .json_body(serde_json::json!({"role_id": "my-role", "secret_id": "my-secret"}));
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"auth":{"client_token":"hvs.test-token","accessor":"","policies":[],"metadata":{},"lease_duration":3600,"renewable":true}}"#);
        });

        let vault_addr = format!("http://{}", server.address());
        let config = HashicorpVaultConfig::new(
            &vault_addr,
            "ai-keys",
            VaultAuth::AppRole {
                role_id: "my-role".to_string(),
                secret_id: "my-secret".to_string(),
            },
        );
        let result = HashicorpVaultStore::new(config).await;
        assert!(result.is_ok());
        mock.assert();
    }
}
