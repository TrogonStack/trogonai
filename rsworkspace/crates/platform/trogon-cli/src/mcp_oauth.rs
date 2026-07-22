//! OAuth 2.1 + PKCE authorization for remote (HTTP) MCP servers.
//!
//! Implements the MCP authorization flow:
//!   1. discover the authorization server from the MCP server's origin
//!      (`.well-known/oauth-protected-resource` → AS metadata),
//!   2. dynamically register a public client (RFC 7591),
//!   3. run the authorization-code + PKCE (S256) flow via the system browser and
//!      a one-shot localhost redirect listener,
//!   4. exchange the code for tokens, and refresh them when they expire.
//!
//! Tokens are persisted in `~/.config/trogon/mcp-auth.json` (never in `mcp.json`)
//! and injected as an `Authorization: Bearer` header when connecting to the MCP
//! server. The browser/callback step is the only part that needs a live user; the
//! protocol mechanics (discovery, registration, token exchange/refresh, PKCE,
//! storage) are unit/`httpmock`-tested.

use base64::Engine as _;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::fs::Fs;

const AUTH_STORE_PATH: &str = "~/.config/trogon/mcp-auth.json";
/// Refresh a token this many seconds before its actual expiry.
const EXPIRY_SKEW_SECS: u64 = 60;
/// How long to wait for the user to complete the browser authorization.
const CALLBACK_TIMEOUT: Duration = Duration::from_secs(300);

fn b64url(bytes: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// `scheme://host[:port]` from `url`, dropping userinfo/path/query/fragment.
fn origin_of(url: &str) -> String {
    let Some(scheme_end) = url.find("://") else {
        return url.to_string();
    };
    let scheme = &url[..scheme_end];
    let after = &url[scheme_end + 3..];
    let authority = match after.rfind('@') {
        Some(at) => &after[at + 1..],
        None => after,
    };
    let host_end = authority.find(['/', '?', '#']).unwrap_or(authority.len());
    format!("{}://{}", scheme, &authority[..host_end])
}

// ── PKCE ────────────────────────────────────────────────────────────────────

/// A PKCE verifier/challenge pair (S256).
#[derive(Debug, Clone)]
pub struct Pkce {
    pub verifier: String,
    pub challenge: String,
}

pub fn generate_pkce() -> Pkce {
    use rand::RngCore;
    let mut buf = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut buf);
    let verifier = b64url(&buf);
    let challenge = b64url(&Sha256::digest(verifier.as_bytes()));
    Pkce { verifier, challenge }
}

pub fn random_state() -> String {
    use rand::RngCore;
    let mut buf = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut buf);
    b64url(&buf)
}

// ── Discovery / wire types ────────────────────────────────────────────────────

#[derive(Debug, Clone, Deserialize)]
struct ProtectedResourceMetadata {
    #[serde(default)]
    authorization_servers: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AuthServerMetadata {
    pub authorization_endpoint: String,
    pub token_endpoint: String,
    #[serde(default)]
    pub registration_endpoint: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RegistrationResponse {
    client_id: String,
    #[serde(default)]
    client_secret: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    expires_in: Option<u64>,
}

// ── Token storage ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct StoredToken {
    pub access_token: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refresh_token: Option<String>,
    /// Absolute unix expiry in seconds; `None` means no known expiry.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<u64>,
    pub token_endpoint: String,
    pub client_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_secret: Option<String>,
}

impl StoredToken {
    /// True when the token is expired or within `skew` secs of expiring.
    pub fn is_expired_at(&self, now: u64, skew: u64) -> bool {
        match self.expires_at {
            Some(exp) => now.saturating_add(skew) >= exp,
            None => false,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct OAuthStore {
    #[serde(default)]
    tokens: HashMap<String, StoredToken>,
}

impl OAuthStore {
    pub fn path() -> PathBuf {
        crate::mcp::expand_tilde(AUTH_STORE_PATH)
    }

    pub fn load<F: Fs>(fs: &F) -> Self {
        match fs.read_to_string(&Self::path()) {
            Ok(raw) if !raw.trim().is_empty() => serde_json::from_str(&raw).unwrap_or_default(),
            _ => Self::default(),
        }
    }

    pub fn save<F: Fs>(&self, fs: &F) -> std::io::Result<()> {
        let path = Self::path();
        if let Some(dir) = path.parent() {
            fs.create_dir_all(dir)?;
        }
        let raw = serde_json::to_string_pretty(self).unwrap_or_else(|_| "{}".into());
        fs.write(&path, raw.as_bytes())
    }

    pub fn get(&self, name: &str) -> Option<&StoredToken> {
        self.tokens.get(name)
    }
    pub fn set(&mut self, name: impl Into<String>, token: StoredToken) {
        self.tokens.insert(name.into(), token);
    }
    pub fn remove(&mut self, name: &str) -> bool {
        self.tokens.remove(name).is_some()
    }
}

// ── HTTP steps (httpmock-testable) ────────────────────────────────────────────

/// Discover the authorization server metadata for an MCP server `mcp_url`.
pub async fn discover(http: &reqwest::Client, mcp_url: &str) -> Result<AuthServerMetadata, String> {
    let origin = origin_of(mcp_url);
    // Protected-resource metadata points at the authorization server(s); if it is
    // absent, fall back to treating the MCP origin as the authorization server.
    let prm_url = format!("{origin}/.well-known/oauth-protected-resource");
    let as_origin = match http.get(&prm_url).send().await {
        Ok(r) if r.status().is_success() => match r.json::<ProtectedResourceMetadata>().await {
            Ok(prm) => prm
                .authorization_servers
                .into_iter()
                .next()
                .map(|s| origin_of(&s))
                .unwrap_or_else(|| origin.clone()),
            Err(_) => origin.clone(),
        },
        _ => origin.clone(),
    };

    for suffix in [
        ".well-known/oauth-authorization-server",
        ".well-known/openid-configuration",
    ] {
        let url = format!("{as_origin}/{suffix}");
        if let Ok(r) = http.get(&url).send().await
            && r.status().is_success()
            && let Ok(meta) = r.json::<AuthServerMetadata>().await
        {
            return Ok(meta);
        }
    }
    Err(format!("could not discover OAuth metadata for {mcp_url}"))
}

/// Dynamically register a public client (RFC 7591).
pub async fn register_client(
    http: &reqwest::Client,
    meta: &AuthServerMetadata,
    redirect_uri: &str,
) -> Result<(String, Option<String>), String> {
    let Some(reg) = &meta.registration_endpoint else {
        return Err("server has no registration endpoint; a pre-registered client_id is required".into());
    };
    let body = serde_json::json!({
        "client_name": "Trogon CLI",
        "redirect_uris": [redirect_uri],
        "grant_types": ["authorization_code", "refresh_token"],
        "response_types": ["code"],
        "token_endpoint_auth_method": "none",
    });
    let r = http
        .post(reg)
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("client registration request failed: {e}"))?;
    if !r.status().is_success() {
        return Err(format!("client registration failed: HTTP {}", r.status()));
    }
    let resp: RegistrationResponse = r
        .json()
        .await
        .map_err(|e| format!("client registration parse error: {e}"))?;
    Ok((resp.client_id, resp.client_secret))
}

/// Build the authorization-endpoint URL with PKCE + state.
pub fn build_authorize_url(
    meta: &AuthServerMetadata,
    client_id: &str,
    redirect_uri: &str,
    challenge: &str,
    state: &str,
    resource: &str,
) -> Result<String, String> {
    let params = [
        ("response_type", "code"),
        ("client_id", client_id),
        ("redirect_uri", redirect_uri),
        ("code_challenge", challenge),
        ("code_challenge_method", "S256"),
        ("state", state),
        ("resource", resource),
    ];
    reqwest::Url::parse_with_params(&meta.authorization_endpoint, params)
        .map(|u| u.to_string())
        .map_err(|e| format!("invalid authorization_endpoint: {e}"))
}

async fn token_request(
    http: &reqwest::Client,
    token_endpoint: &str,
    params: &[(&str, &str)],
) -> Result<TokenResponse, String> {
    let r = http
        .post(token_endpoint)
        .form(params)
        .send()
        .await
        .map_err(|e| format!("token request failed: {e}"))?;
    if !r.status().is_success() {
        let status = r.status();
        let body = r.text().await.unwrap_or_default();
        return Err(format!("token endpoint returned HTTP {status}: {body}"));
    }
    r.json::<TokenResponse>()
        .await
        .map_err(|e| format!("token response parse error: {e}"))
}

async fn exchange_code(
    http: &reqwest::Client,
    token_endpoint: &str,
    client_id: &str,
    code: &str,
    verifier: &str,
    redirect_uri: &str,
) -> Result<TokenResponse, String> {
    token_request(
        http,
        token_endpoint,
        &[
            ("grant_type", "authorization_code"),
            ("code", code),
            ("redirect_uri", redirect_uri),
            ("client_id", client_id),
            ("code_verifier", verifier),
        ],
    )
    .await
}

async fn refresh(
    http: &reqwest::Client,
    token_endpoint: &str,
    client_id: &str,
    refresh_token: &str,
) -> Result<TokenResponse, String> {
    token_request(
        http,
        token_endpoint,
        &[
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token),
            ("client_id", client_id),
        ],
    )
    .await
}

// ── Interactive browser + callback ────────────────────────────────────────────

/// A bound localhost redirect listener. Bind first to learn the port (the
/// redirect URI must be known before registration and the authorize URL), then
/// `wait` for the browser callback.
pub struct CallbackServer {
    listener: tokio::net::TcpListener,
    pub redirect_uri: String,
}

impl CallbackServer {
    pub async fn bind() -> Result<Self, String> {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .map_err(|e| format!("could not bind callback listener: {e}"))?;
        let port = listener.local_addr().map_err(|e| e.to_string())?.port();
        Ok(Self {
            listener,
            redirect_uri: format!("http://127.0.0.1:{port}/callback"),
        })
    }

    /// Serve until the OAuth redirect hits `/callback`, returning the auth code.
    pub async fn wait(self, expected_state: &str) -> Result<String, String> {
        use axum::{Router, extract::Query, routing::get};
        use std::sync::{Arc, Mutex};
        use tokio::sync::oneshot;

        let (tx, rx) = oneshot::channel::<Result<String, String>>();
        let tx = Arc::new(Mutex::new(Some(tx)));
        let want = expected_state.to_string();

        let app = Router::new().route(
            "/callback",
            get(move |Query(params): Query<HashMap<String, String>>| {
                let tx = tx.clone();
                let want = want.clone();
                async move {
                    let result = match (params.get("code"), params.get("state")) {
                        (Some(code), Some(st)) if *st == want => Ok(code.clone()),
                        (_, Some(_)) => Err("state mismatch (possible CSRF) — aborted".to_string()),
                        _ => Err(params
                            .get("error")
                            .cloned()
                            .unwrap_or_else(|| "authorization response missing code".into())),
                    };
                    let ok = result.is_ok();
                    if let Some(tx) = tx.lock().unwrap_or_else(|e| e.into_inner()).take() {
                        let _ = tx.send(result);
                    }
                    if ok {
                        "Authorization complete — you can close this window."
                    } else {
                        "Authorization failed — you can close this window."
                    }
                }
            }),
        );

        let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
        let server = axum::serve(self.listener, app).with_graceful_shutdown(async move {
            let _ = shutdown_rx.await;
        });
        let handle = tokio::spawn(async move {
            let _ = server.await;
        });

        let outcome = match tokio::time::timeout(CALLBACK_TIMEOUT, rx).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => Err("callback channel closed unexpectedly".into()),
            Err(_) => Err("timed out waiting for browser authorization".into()),
        };
        let _ = shutdown_tx.send(());
        let _ = handle.await;
        outcome
    }
}

/// Best-effort launch of the system browser. The URL is also printed so the user
/// can open it manually (headless, SSH, or no default browser).
pub fn open_browser(url: &str) {
    #[cfg(target_os = "macos")]
    let (cmd, args): (&str, Vec<&str>) = ("open", vec![url]);
    #[cfg(target_os = "windows")]
    let (cmd, args): (&str, Vec<&str>) = ("cmd", vec!["/C", "start", "", url]);
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let (cmd, args): (&str, Vec<&str>) = ("xdg-open", vec![url]);

    let _ = std::process::Command::new(cmd)
        .args(args)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
}

// ── Orchestration ─────────────────────────────────────────────────────────────

/// Run the full interactive OAuth flow for `mcp_url` and return the resulting
/// token. Opens the browser and blocks until the user authorizes (or times out).
pub async fn login(http: &reqwest::Client, mcp_url: &str) -> Result<StoredToken, String> {
    login_with(http, mcp_url, |authorize_url| {
        eprintln!("Opening your browser to authorize MCP access. If it doesn't open, visit:");
        eprintln!("  {authorize_url}");
        open_browser(authorize_url);
    })
    .await
}

/// Like [`login`] but with an injectable `open` step (given the authorization
/// URL). Production passes the browser opener; tests pass a driver that follows
/// the redirect, so the whole flow can run end-to-end headlessly.
pub async fn login_with<O: FnOnce(&str)>(
    http: &reqwest::Client,
    mcp_url: &str,
    open: O,
) -> Result<StoredToken, String> {
    let meta = discover(http, mcp_url).await?;
    let callback = CallbackServer::bind().await?;
    // `wait()` consumes `callback`; keep the redirect URI for the token exchange.
    let redirect_uri = callback.redirect_uri.clone();
    let (client_id, client_secret) = register_client(http, &meta, &redirect_uri).await?;
    let pkce = generate_pkce();
    let state = random_state();
    let authorize_url = build_authorize_url(&meta, &client_id, &redirect_uri, &pkce.challenge, &state, mcp_url)?;

    open(&authorize_url);

    let code = callback.wait(&state).await?;
    let tok = exchange_code(
        http,
        &meta.token_endpoint,
        &client_id,
        &code,
        &pkce.verifier,
        &redirect_uri,
    )
    .await?;

    Ok(StoredToken {
        access_token: tok.access_token,
        refresh_token: tok.refresh_token,
        expires_at: tok.expires_in.map(|e| now_secs().saturating_add(e)),
        token_endpoint: meta.token_endpoint,
        client_id,
        client_secret,
    })
}

/// Return a valid access token for `name`, refreshing (and persisting) it if it
/// has expired. Errors if there is no stored token or the refresh fails.
pub async fn ensure_token<F: Fs>(
    name: &str,
    store: &mut OAuthStore,
    fs: &F,
    http: &reqwest::Client,
) -> Result<String, String> {
    let Some(tok) = store.get(name).cloned() else {
        return Err(format!("no OAuth token for `{name}` — run /mcp login {name}"));
    };
    if !tok.is_expired_at(now_secs(), EXPIRY_SKEW_SECS) {
        return Ok(tok.access_token);
    }
    let Some(rt) = tok.refresh_token.as_deref() else {
        return Err(format!(
            "OAuth token for `{name}` expired and has no refresh token — run /mcp login {name}"
        ));
    };
    let refreshed = refresh(http, &tok.token_endpoint, &tok.client_id, rt).await?;
    let updated = StoredToken {
        access_token: refreshed.access_token,
        refresh_token: refreshed.refresh_token.or(tok.refresh_token),
        expires_at: refreshed.expires_in.map(|e| now_secs().saturating_add(e)),
        token_endpoint: tok.token_endpoint,
        client_id: tok.client_id,
        client_secret: tok.client_secret,
    };
    let access = updated.access_token.clone();
    store.set(name, updated);
    store
        .save(fs)
        .map_err(|e| format!("could not save OAuth token store: {e}"))?;
    Ok(access)
}

#[cfg(test)]
mod tests {
    use super::*;
    use httpmock::prelude::*;

    #[test]
    fn pkce_challenge_is_sha256_base64url_of_verifier() {
        let p = generate_pkce();
        // Recompute the challenge from the verifier and compare.
        let expect = b64url(&Sha256::digest(p.verifier.as_bytes()));
        assert_eq!(p.challenge, expect);
        // base64url-no-pad: no '+', '/', or '='.
        assert!(!p.verifier.contains(['+', '/', '=']));
        assert!(!p.challenge.contains(['+', '/', '=']));
    }

    #[test]
    fn origin_strips_path_and_userinfo() {
        assert_eq!(
            origin_of("https://u:p@mcp.example.com:8443/mcp?x=1"),
            "https://mcp.example.com:8443"
        );
        assert_eq!(origin_of("http://localhost:3000/sse"), "http://localhost:3000");
    }

    #[test]
    fn expiry_uses_skew() {
        let tok = StoredToken {
            access_token: "a".into(),
            refresh_token: None,
            expires_at: Some(1000),
            token_endpoint: "t".into(),
            client_id: "c".into(),
            client_secret: None,
        };
        assert!(!tok.is_expired_at(900, 60)); // 960 < 1000
        assert!(tok.is_expired_at(950, 60)); // 1010 >= 1000
        // No expiry → never expired.
        let mut t2 = tok.clone();
        t2.expires_at = None;
        assert!(!t2.is_expired_at(u64::MAX - 1, 60));
    }

    #[test]
    fn build_authorize_url_encodes_params() {
        let meta = AuthServerMetadata {
            authorization_endpoint: "https://as.example.com/authorize".into(),
            token_endpoint: "https://as.example.com/token".into(),
            registration_endpoint: None,
        };
        let url = build_authorize_url(
            &meta,
            "client123",
            "http://127.0.0.1:5000/callback",
            "chal",
            "state1",
            "https://mcp.example.com/mcp",
        )
        .unwrap();
        assert!(url.starts_with("https://as.example.com/authorize?"));
        assert!(url.contains("code_challenge_method=S256"));
        assert!(url.contains("client_id=client123"));
        // redirect_uri must be percent-encoded.
        assert!(url.contains("redirect_uri=http%3A%2F%2F127.0.0.1%3A5000%2Fcallback"));
        assert!(url.contains("resource=https%3A%2F%2Fmcp.example.com%2Fmcp"));
    }

    #[test]
    fn store_round_trips_via_mock_fs() {
        use crate::fs::mock::MockFs;
        let fs = MockFs::new();
        let mut store = OAuthStore::default();
        store.set(
            "linear",
            StoredToken {
                access_token: "tok".into(),
                refresh_token: Some("r".into()),
                expires_at: Some(123),
                token_endpoint: "https://as/token".into(),
                client_id: "cid".into(),
                client_secret: None,
            },
        );
        store.save(&fs).unwrap();
        let loaded = OAuthStore::load(&fs);
        assert_eq!(loaded.get("linear"), store.get("linear"));
    }

    #[tokio::test]
    async fn discover_reads_protected_resource_then_as_metadata() {
        let server = MockServer::start();
        let prm = server.mock(|when, then| {
            when.method(GET).path("/.well-known/oauth-protected-resource");
            then.status(200).json_body(serde_json::json!({
                "authorization_servers": [server.base_url()]
            }));
        });
        let asm = server.mock(|when, then| {
            when.method(GET).path("/.well-known/oauth-authorization-server");
            then.status(200).json_body(serde_json::json!({
                "authorization_endpoint": format!("{}/authorize", server.base_url()),
                "token_endpoint": format!("{}/token", server.base_url()),
                "registration_endpoint": format!("{}/register", server.base_url()),
            }));
        });

        let http = reqwest::Client::new();
        let meta = discover(&http, &format!("{}/mcp", server.base_url())).await.unwrap();
        prm.assert();
        asm.assert();
        assert!(meta.token_endpoint.ends_with("/token"));
        assert!(meta.registration_endpoint.is_some());
    }

    #[tokio::test]
    async fn exchange_and_refresh_parse_tokens() {
        let server = MockServer::start();
        let token_ep = format!("{}/token", server.base_url());

        let exchange = server.mock(|when, then| {
            when.method(POST).path("/token").body_contains("authorization_code");
            then.status(200).json_body(serde_json::json!({
                "access_token": "access-1", "refresh_token": "refresh-1",
                "token_type": "Bearer", "expires_in": 3600
            }));
        });
        let http = reqwest::Client::new();
        let tok = exchange_code(&http, &token_ep, "cid", "code", "verifier", "http://127.0.0.1/cb")
            .await
            .unwrap();
        exchange.assert();
        assert_eq!(tok.access_token, "access-1");
        assert_eq!(tok.expires_in, Some(3600));

        let refresh_mock = server.mock(|when, then| {
            when.method(POST).path("/token").body_contains("refresh_token");
            then.status(200).json_body(serde_json::json!({
                "access_token": "access-2", "token_type": "Bearer", "expires_in": 60
            }));
        });
        let tok2 = refresh(&http, &token_ep, "cid", "refresh-1").await.unwrap();
        refresh_mock.assert();
        assert_eq!(tok2.access_token, "access-2");
    }
}
