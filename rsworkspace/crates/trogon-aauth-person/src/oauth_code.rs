//! OAuth 2.1 `authorization_code` grant with PKCE-S256.
//!
//! Per `GCP_TODO.md §3.6`:
//! - PKCE **required**, S256 only. Plain is rejected.
//! - No refresh tokens in v0.1.
//! - Client registration is record-based via [`crate::oauth_clients`].
//! - Consent goes through [`crate::policy::ConsentPolicy::decide`].

use std::collections::HashMap;
use std::sync::Mutex;

use async_trait::async_trait;
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use sha2::{Digest, Sha256};
use serde::{Deserialize, Serialize};

pub const PKCE_METHOD_S256: &str = "S256";
pub const GRANT_TYPE_AUTHORIZATION_CODE: &str = "authorization_code";
pub const RESPONSE_TYPE_CODE: &str = "code";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthorizationCodeRecord {
    pub code: String,
    pub client_id: String,
    pub redirect_uri: String,
    pub principal: String,
    pub scope: String,
    pub code_challenge: String,
    pub code_challenge_method: String,
    pub issued_at: i64,
    pub expires_at: i64,
}

#[derive(Debug)]
pub enum AuthorizationCodeStoreError {
    Backend(String),
}

impl std::fmt::Display for AuthorizationCodeStoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Backend(message) => write!(f, "authorization code store: {message}"),
        }
    }
}

impl std::error::Error for AuthorizationCodeStoreError {}

#[async_trait]
pub trait AuthorizationCodeStore: Send + Sync {
    async fn put(&self, record: AuthorizationCodeRecord) -> Result<(), AuthorizationCodeStoreError>;
    /// Atomically read-and-delete; authorization codes are one-shot.
    async fn take(&self, code: &str) -> Result<Option<AuthorizationCodeRecord>, AuthorizationCodeStoreError>;
}

#[derive(Default)]
pub struct InMemoryAuthorizationCodeStore {
    records: Mutex<HashMap<String, AuthorizationCodeRecord>>,
}

impl InMemoryAuthorizationCodeStore {
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl AuthorizationCodeStore for InMemoryAuthorizationCodeStore {
    async fn put(&self, record: AuthorizationCodeRecord) -> Result<(), AuthorizationCodeStoreError> {
        let mut guard = self.records.lock().expect("authz code store lock");
        guard.insert(record.code.clone(), record);
        Ok(())
    }

    async fn take(&self, code: &str) -> Result<Option<AuthorizationCodeRecord>, AuthorizationCodeStoreError> {
        let mut guard = self.records.lock().expect("authz code store lock");
        Ok(guard.remove(code))
    }
}

/// PKCE-S256: `BASE64URL(SHA256(code_verifier)) == code_challenge`.
/// Returns `false` if the verifier is malformed (out-of-spec length).
pub fn verify_pkce_s256(code_verifier: &str, code_challenge: &str) -> bool {
    let len = code_verifier.len();
    if !(43..=128).contains(&len) {
        return false;
    }
    if !code_verifier
        .bytes()
        .all(|b| matches!(b, b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~'))
    {
        return false;
    }
    let digest = Sha256::digest(code_verifier.as_bytes());
    URL_SAFE_NO_PAD.encode(digest) == code_challenge
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthorizeRequest {
    pub response_type: String,
    pub client_id: String,
    pub redirect_uri: String,
    pub code_challenge: String,
    pub code_challenge_method: String,
    #[serde(default)]
    pub scope: String,
    #[serde(default)]
    pub state: Option<String>,
    pub principal: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthorizeResponse {
    pub code: String,
    pub state: Option<String>,
    pub redirect_uri: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenExchangeRequest {
    pub grant_type: String,
    pub code: String,
    pub redirect_uri: String,
    pub client_id: String,
    pub code_verifier: String,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum OAuthError {
    #[error("unsupported response_type `{0}`")]
    UnsupportedResponseType(String),
    #[error("unsupported grant_type `{0}`")]
    UnsupportedGrantType(String),
    #[error("only PKCE S256 is supported, got `{0}`")]
    UnsupportedPkceMethod(String),
    #[error("unknown client `{0}`")]
    UnknownClient(String),
    #[error("client `{0}` is disabled")]
    ClientDisabled(String),
    #[error("redirect_uri not registered for client `{0}`")]
    RedirectUriMismatch(String),
    #[error("requested scope is not allowed for this client")]
    ScopeNotAllowed,
    #[error("authorization code not found")]
    CodeNotFound,
    #[error("authorization code has expired")]
    CodeExpired,
    #[error("authorization code does not match client")]
    CodeClientMismatch,
    #[error("authorization code does not match redirect_uri")]
    CodeRedirectMismatch,
    #[error("PKCE verification failed")]
    PkceVerificationFailed,
    #[error("consent denied: {0}")]
    ConsentDenied(String),
    #[error("requires interaction at {url}")]
    RequiresInteraction { url: String, code: Option<String> },
    #[error("store: {0}")]
    Store(String),
}

/// Issued `aa-auth+jwt` plus echoed scope/ttl, mirroring [`crate::core::TokenResponse`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OAuthTokenResponse {
    pub access_token: String,
    pub token_type: String,
    pub expires_in: i64,
    pub scope: String,
}

/// Result of an authorization flow ready to be persisted + redirected.
pub struct AuthorizedCode {
    pub record: AuthorizationCodeRecord,
}

pub struct OAuthCodeServiceConfig {
    pub code_ttl_secs: i64,
}

impl Default for OAuthCodeServiceConfig {
    fn default() -> Self {
        Self { code_ttl_secs: 60 }
    }
}

/// Pure validation step for an authorize request. The caller (typically
/// the HTTP handler) then runs the consent decision and persists the
/// resulting code record. Splitting this out keeps async/IO concerns
/// out of the validation surface and makes unit testing trivial.
pub fn validate_authorize_request(
    req: &AuthorizeRequest,
    client: &crate::oauth_clients::OAuthClientRecord,
) -> Result<(), OAuthError> {
    if req.response_type != RESPONSE_TYPE_CODE {
        return Err(OAuthError::UnsupportedResponseType(req.response_type.clone()));
    }
    if req.code_challenge_method != PKCE_METHOD_S256 {
        return Err(OAuthError::UnsupportedPkceMethod(req.code_challenge_method.clone()));
    }
    if !client.is_enabled() {
        return Err(OAuthError::ClientDisabled(req.client_id.clone()));
    }
    if !client.allows_redirect(&req.redirect_uri) {
        return Err(OAuthError::RedirectUriMismatch(req.client_id.clone()));
    }
    if !req.scope.is_empty() && !client.allows_scope(&req.scope) {
        return Err(OAuthError::ScopeNotAllowed);
    }
    Ok(())
}

/// Pure validation step for a token exchange. Verifies PKCE, the
/// client/redirect bindings, and the code's freshness. The caller is
/// responsible for `take`-ing the code from the store (one-shot).
pub fn validate_token_exchange(
    req: &TokenExchangeRequest,
    record: &AuthorizationCodeRecord,
    now: i64,
) -> Result<(), OAuthError> {
    if req.grant_type != GRANT_TYPE_AUTHORIZATION_CODE {
        return Err(OAuthError::UnsupportedGrantType(req.grant_type.clone()));
    }
    if record.client_id != req.client_id {
        return Err(OAuthError::CodeClientMismatch);
    }
    if record.redirect_uri != req.redirect_uri {
        return Err(OAuthError::CodeRedirectMismatch);
    }
    if now > record.expires_at {
        return Err(OAuthError::CodeExpired);
    }
    if record.code_challenge_method != PKCE_METHOD_S256 {
        return Err(OAuthError::UnsupportedPkceMethod(record.code_challenge_method.clone()));
    }
    if !verify_pkce_s256(&req.code_verifier, &record.code_challenge) {
        return Err(OAuthError::PkceVerificationFailed);
    }
    Ok(())
}

/// Generate a fresh authorization code. Uses a 256-bit value encoded
/// as URL-safe base64 (43 chars, no padding).
pub fn fresh_code(seed: &[u8]) -> String {
    let digest = Sha256::digest(seed);
    URL_SAFE_NO_PAD.encode(digest)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn challenge_for(verifier: &str) -> String {
        let digest = Sha256::digest(verifier.as_bytes());
        URL_SAFE_NO_PAD.encode(digest)
    }

    #[test]
    fn pkce_s256_round_trip() {
        let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        let challenge = challenge_for(verifier);
        assert!(verify_pkce_s256(verifier, &challenge));
    }

    #[test]
    fn pkce_rejects_wrong_verifier() {
        let challenge = challenge_for("the-real-one-and-only-verifier-43-chars!");
        assert!(!verify_pkce_s256("not-the-right-verifier-at-all-43-chars!!", &challenge));
    }

    #[test]
    fn pkce_rejects_too_short_verifier() {
        assert!(!verify_pkce_s256("short", "anything"));
    }

    #[test]
    fn pkce_rejects_too_long_verifier() {
        let too_long = "a".repeat(129);
        assert!(!verify_pkce_s256(&too_long, "anything"));
    }

    #[test]
    fn pkce_rejects_invalid_chars() {
        let bad = "!".repeat(43);
        assert!(!verify_pkce_s256(&bad, "anything"));
    }

    fn sample_client() -> crate::oauth_clients::OAuthClientRecord {
        crate::oauth_clients::OAuthClientRecord {
            client_id: "cursor-cli".into(),
            redirect_uris: vec!["http://127.0.0.1:8765/callback".into()],
            allowed_scopes: vec!["read:tools".into()],
            lifecycle_state: crate::oauth_clients::OAuthClientLifecycleState::Enabled,
            created_at: 0,
            updated_at: 0,
        }
    }

    fn sample_authorize() -> AuthorizeRequest {
        AuthorizeRequest {
            response_type: RESPONSE_TYPE_CODE.into(),
            client_id: "cursor-cli".into(),
            redirect_uri: "http://127.0.0.1:8765/callback".into(),
            code_challenge: "ch".into(),
            code_challenge_method: PKCE_METHOD_S256.into(),
            scope: "read:tools".into(),
            state: Some("xyz".into()),
            principal: "alice".into(),
        }
    }

    #[test]
    fn authorize_validation_happy_path() {
        validate_authorize_request(&sample_authorize(), &sample_client()).expect("ok");
    }

    #[test]
    fn authorize_rejects_plain_pkce() {
        let mut req = sample_authorize();
        req.code_challenge_method = "plain".into();
        let err = validate_authorize_request(&req, &sample_client()).unwrap_err();
        assert!(matches!(err, OAuthError::UnsupportedPkceMethod(_)));
    }

    #[test]
    fn authorize_rejects_unregistered_redirect() {
        let mut req = sample_authorize();
        req.redirect_uri = "https://attacker.example/callback".into();
        let err = validate_authorize_request(&req, &sample_client()).unwrap_err();
        assert!(matches!(err, OAuthError::RedirectUriMismatch(_)));
    }

    #[test]
    fn authorize_rejects_disabled_client() {
        let mut client = sample_client();
        client.lifecycle_state = crate::oauth_clients::OAuthClientLifecycleState::Disabled;
        let err = validate_authorize_request(&sample_authorize(), &client).unwrap_err();
        assert!(matches!(err, OAuthError::ClientDisabled(_)));
    }

    #[test]
    fn authorize_rejects_unsupported_response_type() {
        let mut req = sample_authorize();
        req.response_type = "token".into();
        let err = validate_authorize_request(&req, &sample_client()).unwrap_err();
        assert!(matches!(err, OAuthError::UnsupportedResponseType(_)));
    }

    #[test]
    fn authorize_rejects_out_of_scope() {
        let mut req = sample_authorize();
        req.scope = "admin:secrets".into();
        let err = validate_authorize_request(&req, &sample_client()).unwrap_err();
        assert!(matches!(err, OAuthError::ScopeNotAllowed));
    }

    fn sample_record_for(verifier: &str) -> AuthorizationCodeRecord {
        AuthorizationCodeRecord {
            code: "the-code".into(),
            client_id: "cursor-cli".into(),
            redirect_uri: "http://127.0.0.1:8765/callback".into(),
            principal: "alice".into(),
            scope: "read:tools".into(),
            code_challenge: challenge_for(verifier),
            code_challenge_method: PKCE_METHOD_S256.into(),
            issued_at: 0,
            expires_at: 1_000,
        }
    }

    fn sample_token_exchange(verifier: &str) -> TokenExchangeRequest {
        TokenExchangeRequest {
            grant_type: GRANT_TYPE_AUTHORIZATION_CODE.into(),
            code: "the-code".into(),
            redirect_uri: "http://127.0.0.1:8765/callback".into(),
            client_id: "cursor-cli".into(),
            code_verifier: verifier.into(),
        }
    }

    #[test]
    fn token_exchange_happy_path() {
        let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        validate_token_exchange(&sample_token_exchange(verifier), &sample_record_for(verifier), 100)
            .expect("ok");
    }

    #[test]
    fn token_exchange_rejects_bad_verifier() {
        let real = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        let attacker = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";
        let err = validate_token_exchange(
            &sample_token_exchange(attacker),
            &sample_record_for(real),
            100,
        )
        .unwrap_err();
        assert_eq!(err, OAuthError::PkceVerificationFailed);
    }

    #[test]
    fn token_exchange_rejects_expired_code() {
        let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        let err = validate_token_exchange(
            &sample_token_exchange(verifier),
            &sample_record_for(verifier),
            10_000,
        )
        .unwrap_err();
        assert_eq!(err, OAuthError::CodeExpired);
    }

    #[test]
    fn token_exchange_rejects_redirect_swap() {
        let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        let mut req = sample_token_exchange(verifier);
        req.redirect_uri = "https://attacker.example/callback".into();
        let err = validate_token_exchange(&req, &sample_record_for(verifier), 100).unwrap_err();
        assert_eq!(err, OAuthError::CodeRedirectMismatch);
    }

    #[test]
    fn token_exchange_rejects_client_swap() {
        let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        let mut req = sample_token_exchange(verifier);
        req.client_id = "evil-cli".into();
        let err = validate_token_exchange(&req, &sample_record_for(verifier), 100).unwrap_err();
        assert_eq!(err, OAuthError::CodeClientMismatch);
    }

    #[test]
    fn token_exchange_rejects_wrong_grant_type() {
        let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        let mut req = sample_token_exchange(verifier);
        req.grant_type = "password".into();
        let err = validate_token_exchange(&req, &sample_record_for(verifier), 100).unwrap_err();
        assert!(matches!(err, OAuthError::UnsupportedGrantType(_)));
    }

    #[tokio::test]
    async fn code_store_is_one_shot() {
        let store = InMemoryAuthorizationCodeStore::new();
        let record = AuthorizationCodeRecord {
            code: "abc".into(),
            client_id: "cid".into(),
            redirect_uri: "https://x".into(),
            principal: "alice".into(),
            scope: "".into(),
            code_challenge: "ch".into(),
            code_challenge_method: PKCE_METHOD_S256.into(),
            issued_at: 0,
            expires_at: 999,
        };
        store.put(record.clone()).await.unwrap();
        assert_eq!(store.take("abc").await.unwrap(), Some(record));
        assert_eq!(store.take("abc").await.unwrap(), None);
    }
}
