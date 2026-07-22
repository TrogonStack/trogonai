//! End-to-end test of the MCP OAuth flow against a real (local) spec-compliant
//! authorization server. This drives trogon's actual OAuth code — discovery,
//! dynamic client registration, the PKCE authorize+redirect, the localhost
//! callback capture, token exchange, and refresh — over real HTTP. The only
//! substitution for a human is that the fake `/authorize` endpoint auto-consents
//! (immediately redirecting to the callback); it still VERIFIES the PKCE
//! code_challenge against the submitted code_verifier, so an incorrect PKCE
//! implementation would fail this test.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use axum::{
    Json, Router,
    extract::{Form, Query, State},
    response::{IntoResponse, Redirect},
    routing::{get, post},
};
use base64::Engine as _;
use sha2::{Digest, Sha256};

use trogon_cli::mcp_oauth::{OAuthStore, ensure_token, login_with};

struct AsState {
    base: String,
    /// code_challenge captured at /authorize, verified at /token.
    challenge: Mutex<Option<String>>,
}

fn b64url(b: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(b)
}

async fn protected_resource(State(s): State<Arc<AsState>>) -> impl IntoResponse {
    Json(serde_json::json!({ "authorization_servers": [s.base] }))
}

async fn as_metadata(State(s): State<Arc<AsState>>) -> impl IntoResponse {
    Json(serde_json::json!({
        "authorization_endpoint": format!("{}/authorize", s.base),
        "token_endpoint": format!("{}/token", s.base),
        "registration_endpoint": format!("{}/register", s.base),
    }))
}

async fn register() -> impl IntoResponse {
    Json(serde_json::json!({ "client_id": "test-client" }))
}

async fn authorize(State(s): State<Arc<AsState>>, Query(q): Query<HashMap<String, String>>) -> impl IntoResponse {
    // Capture PKCE challenge to verify at /token; auto-consent by redirecting back.
    *s.challenge.lock().unwrap() = q.get("code_challenge").cloned();
    let redirect_uri = q.get("redirect_uri").cloned().unwrap_or_default();
    let state = q.get("state").cloned().unwrap_or_default();
    Redirect::to(&format!("{redirect_uri}?code=authcode-123&state={state}"))
}

async fn token(State(s): State<Arc<AsState>>, Form(form): Form<HashMap<String, String>>) -> impl IntoResponse {
    match form.get("grant_type").map(String::as_str) {
        Some("authorization_code") => {
            // Verify PKCE: base64url(sha256(verifier)) must equal the stored challenge.
            let verifier = form.get("code_verifier").cloned().unwrap_or_default();
            let expected = b64url(&Sha256::digest(verifier.as_bytes()));
            let challenge = s.challenge.lock().unwrap().clone().unwrap_or_default();
            if expected != challenge {
                return (
                    axum::http::StatusCode::BAD_REQUEST,
                    Json(serde_json::json!({ "error": "invalid_grant", "reason": "pkce mismatch" })),
                );
            }
            (
                axum::http::StatusCode::OK,
                Json(serde_json::json!({
                    "access_token": "access-token-1",
                    "refresh_token": "refresh-token-1",
                    "token_type": "Bearer",
                    "expires_in": 3600
                })),
            )
        }
        Some("refresh_token") => (
            axum::http::StatusCode::OK,
            Json(serde_json::json!({
                "access_token": "access-token-2",
                "token_type": "Bearer",
                "expires_in": 3600
            })),
        ),
        _ => (
            axum::http::StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "unsupported_grant_type" })),
        ),
    }
}

/// Start the fake authorization+resource server; returns its base URL.
async fn start_fake_as() -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let base = format!("http://127.0.0.1:{port}");
    let state = Arc::new(AsState {
        base: base.clone(),
        challenge: Mutex::new(None),
    });
    let app = Router::new()
        .route("/.well-known/oauth-protected-resource", get(protected_resource))
        .route("/.well-known/oauth-authorization-server", get(as_metadata))
        .route("/register", post(register))
        .route("/authorize", get(authorize))
        .route("/token", post(token))
        .with_state(state);
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    base
}

#[tokio::test]
async fn oauth_login_and_refresh_end_to_end() {
    let base = start_fake_as().await;
    let mcp_url = format!("{base}/mcp");
    let http = reqwest::Client::new();

    // Drive the flow. The "browser" is a redirect-following GET of the authorize
    // URL, which lands on trogon's localhost callback with the auth code.
    let browser_http = http.clone();
    let token = login_with(&http, &mcp_url, move |authorize_url| {
        let url = authorize_url.to_string();
        let client = browser_http.clone();
        tokio::spawn(async move {
            // reqwest follows the 3xx redirect to the localhost callback by default.
            let _ = client.get(&url).send().await;
        });
    })
    .await
    .expect("login should succeed against the local AS");

    assert_eq!(token.access_token, "access-token-1");
    assert_eq!(token.refresh_token.as_deref(), Some("refresh-token-1"));
    assert!(token.expires_at.is_some(), "expires_in should yield an absolute expiry");
    assert_eq!(token.client_id, "test-client");

    // Now force expiry and confirm ensure_token refreshes via the refresh grant.
    let fs = trogon_cli::RealFs;
    let mut store = OAuthStore::default();
    let mut expired = token.clone();
    expired.expires_at = Some(1); // far in the past
    store.set("e2e", expired);

    // Use a temp HOME so we don't touch the real ~/.config during save.
    let tmp = std::env::temp_dir().join(format!("trogon_oauth_e2e_{}", std::process::id()));
    std::fs::create_dir_all(&tmp).unwrap();
    // SAFETY: single-threaded test setup; set HOME so the store writes under tmp.
    unsafe {
        std::env::set_var("HOME", &tmp);
    }

    let access = ensure_token("e2e", &mut store, &fs, &http)
        .await
        .expect("ensure_token should refresh the expired token");
    assert_eq!(access, "access-token-2", "expired token must be refreshed");
}
