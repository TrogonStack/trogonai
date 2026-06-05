//! Axum router serving the Person Server HTTP endpoints.

use std::sync::Arc;

use axum::{
    Json, Router,
    extract::State,
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
};
use jsonwebtoken::jwk::JwkSet;
use trogon_aauth_verify::{JwksResolver, time_source::TimeSource};

use crate::core::{BootstrapRequest, PersonCore, PersonError, TokenRequest};
use crate::oauth_clients::OAuthClientStore;
use crate::oauth_code::{
    AuthorizationCodeRecord, AuthorizationCodeStore, AuthorizeRequest, AuthorizeResponse, OAuthCodeServiceConfig,
    OAuthError, OAuthTokenResponse, TokenExchangeRequest, fresh_code, validate_authorize_request,
    validate_token_exchange,
};
use crate::policy::{ConsentContext, ConsentDecision, ConsentPolicy};
use crate::store::PersonStore;
use crate::wif::WifStore;
use crate::wif_exchange::{WifExchangeError, WifExchangeRequest, WifExchangeService};

pub struct HttpState<R: JwksResolver, S: PersonStore, P: ConsentPolicy, C: TimeSource> {
    pub core: Arc<PersonCore<R, S, P, C>>,
    pub jwks: JwkSet,
}

pub fn router<R, S, P, C>(state: Arc<HttpState<R, S, P, C>>) -> Router
where
    R: JwksResolver + 'static,
    S: PersonStore + 'static,
    P: ConsentPolicy + 'static,
    C: TimeSource + Clone + 'static,
{
    Router::new()
        .route("/.well-known/jwks.json", get(get_jwks::<R, S, P, C>))
        .route("/.well-known/aauth-person.json", get(get_meta::<R, S, P, C>))
        .route("/aauth/agent", post(post_bootstrap::<R, S, P, C>))
        .route("/aauth/token", post(post_token::<R, S, P, C>))
        .with_state(state)
}

pub struct WifHttpState<R, S, P, C, W, J>
where
    R: JwksResolver,
    S: PersonStore,
    P: ConsentPolicy,
    C: TimeSource,
    W: WifStore,
    J: JwksResolver,
{
    pub core: Arc<PersonCore<R, S, P, C>>,
    pub exchange: Arc<WifExchangeService<Arc<W>, J, C>>,
}

pub fn wif_router<R, S, P, C, W, J>(state: Arc<WifHttpState<R, S, P, C, W, J>>) -> Router
where
    R: JwksResolver + 'static,
    S: PersonStore + 'static,
    P: ConsentPolicy + 'static,
    C: TimeSource + Clone + 'static,
    W: WifStore + 'static,
    J: JwksResolver + 'static,
{
    Router::new()
        .route("/aauth/wif/exchange", post(post_wif_exchange::<R, S, P, C, W, J>))
        .with_state(state)
}

async fn get_jwks<R, S, P, C>(State(s): State<Arc<HttpState<R, S, P, C>>>) -> Json<JwkSet>
where
    R: JwksResolver,
    S: PersonStore,
    P: ConsentPolicy,
    C: TimeSource,
{
    Json(s.jwks.clone())
}

#[derive(serde::Serialize)]
struct Metadata {
    issuer: String,
    jwks_uri: String,
    token_endpoint: String,
    bootstrap_endpoint: String,
}

async fn get_meta<R, S, P, C>(State(s): State<Arc<HttpState<R, S, P, C>>>) -> Json<Metadata>
where
    R: JwksResolver,
    S: PersonStore,
    P: ConsentPolicy,
    C: TimeSource,
{
    let iss = s.core.iss.clone();
    Json(Metadata {
        issuer: iss.clone(),
        jwks_uri: format!("{iss}/.well-known/jwks.json"),
        token_endpoint: format!("{iss}/aauth/token"),
        bootstrap_endpoint: format!("{iss}/aauth/agent"),
    })
}

async fn post_bootstrap<R, S, P, C>(
    State(s): State<Arc<HttpState<R, S, P, C>>>,
    Json(req): Json<BootstrapRequest>,
) -> impl IntoResponse
where
    R: JwksResolver,
    S: PersonStore,
    P: ConsentPolicy,
    C: TimeSource,
{
    match s.core.bootstrap(req).await {
        Ok(resp) => Json(resp).into_response(),
        Err(e) => person_error_to_http(e),
    }
}

async fn post_token<R, S, P, C>(
    State(s): State<Arc<HttpState<R, S, P, C>>>,
    Json(req): Json<TokenRequest>,
) -> impl IntoResponse
where
    R: JwksResolver,
    S: PersonStore,
    P: ConsentPolicy,
    C: TimeSource,
{
    match s.core.exchange(req).await {
        Ok(resp) => Json(resp).into_response(),
        Err(e) => person_error_to_http(e),
    }
}

async fn post_wif_exchange<R, S, P, C, W, J>(
    State(s): State<Arc<WifHttpState<R, S, P, C, W, J>>>,
    Json(req): Json<WifExchangeRequest>,
) -> impl IntoResponse
where
    R: JwksResolver,
    S: PersonStore,
    P: ConsentPolicy,
    C: TimeSource,
    W: WifStore,
    J: JwksResolver,
{
    let resolved = match s.exchange.resolve(&req).await {
        Ok(r) => r,
        Err(e) => return wif_error_to_http(e),
    };
    match s.core.mint_wif_auth(&resolved) {
        Ok(resp) => Json(resp).into_response(),
        Err(e) => person_error_to_http(e),
    }
}

fn wif_error_to_http(e: WifExchangeError) -> axum::response::Response {
    let code = match &e {
        WifExchangeError::UnsupportedGrantType(_) | WifExchangeError::UnsupportedSubjectTokenType(_) => {
            StatusCode::BAD_REQUEST
        }
        WifExchangeError::UnknownProvider(_)
        | WifExchangeError::ProviderDisabled(_)
        | WifExchangeError::UnknownServiceAccount(..)
        | WifExchangeError::NoMappingMatch
        | WifExchangeError::AmbiguousMatch(_) => StatusCode::FORBIDDEN,
        WifExchangeError::SubjectToken(_) | WifExchangeError::IssuerMismatch { .. } => StatusCode::UNAUTHORIZED,
        WifExchangeError::Jwks(_) | WifExchangeError::ClaimMapping(_) | WifExchangeError::Store(_) => {
            StatusCode::INTERNAL_SERVER_ERROR
        }
    };
    (code, e.to_string()).into_response()
}

pub struct OAuthHttpState<R, S, P, C, K, A>
where
    R: JwksResolver,
    S: PersonStore,
    P: ConsentPolicy,
    C: TimeSource,
    K: OAuthClientStore,
    A: AuthorizationCodeStore,
{
    pub core: Arc<PersonCore<R, S, P, C>>,
    pub clients: Arc<K>,
    pub codes: Arc<A>,
    pub config: OAuthCodeServiceConfig,
}

pub fn oauth_router<R, S, P, C, K, A>(state: Arc<OAuthHttpState<R, S, P, C, K, A>>) -> Router
where
    R: JwksResolver + 'static,
    S: PersonStore + 'static,
    P: ConsentPolicy + 'static,
    C: TimeSource + Clone + 'static,
    K: OAuthClientStore + 'static,
    A: AuthorizationCodeStore + 'static,
{
    Router::new()
        .route("/aauth/oauth/authorize", post(post_authorize::<R, S, P, C, K, A>))
        .route("/aauth/oauth/token", post(post_oauth_token::<R, S, P, C, K, A>))
        .with_state(state)
}

async fn post_authorize<R, S, P, C, K, A>(
    State(s): State<Arc<OAuthHttpState<R, S, P, C, K, A>>>,
    Json(req): Json<AuthorizeRequest>,
) -> impl IntoResponse
where
    R: JwksResolver,
    S: PersonStore,
    P: ConsentPolicy,
    C: TimeSource,
    K: OAuthClientStore,
    A: AuthorizationCodeStore,
{
    let client = match s.clients.get(&req.client_id).await {
        Ok(Some(c)) => c,
        Ok(None) => return oauth_error_to_http(OAuthError::UnknownClient(req.client_id.clone())),
        Err(e) => return oauth_error_to_http(OAuthError::Store(e.to_string())),
    };
    if let Err(e) = validate_authorize_request(&req, &client) {
        return oauth_error_to_http(e);
    }
    let decision = s
        .core
        .policy
        .decide(&ConsentContext {
            principal: &req.principal,
            agent_id: &req.client_id,
            resource_iss: &s.core.iss,
            requested_scope: &req.scope,
            backend: req.backend.as_deref().unwrap_or(&req.client_id),
        })
        .await;
    let granted_scope = match decision {
        ConsentDecision::Allow { granted_scope, .. } => granted_scope,
        ConsentDecision::Interaction { url, code } => {
            return oauth_error_to_http(OAuthError::RequiresInteraction { url, code });
        }
        ConsentDecision::Deny { reason } => return oauth_error_to_http(OAuthError::ConsentDenied(reason)),
    };
    let now = s.core.clock.now();
    let seed = format!("{}:{}:{}:{}", req.client_id, req.principal, req.code_challenge, now);
    let code = fresh_code(seed.as_bytes());
    let record = AuthorizationCodeRecord {
        code: code.clone(),
        client_id: req.client_id.clone(),
        redirect_uri: req.redirect_uri.clone(),
        principal: req.principal.clone(),
        scope: granted_scope,
        code_challenge: req.code_challenge.clone(),
        code_challenge_method: req.code_challenge_method.clone(),
        issued_at: now,
        expires_at: now + s.config.code_ttl_secs,
    };
    if let Err(e) = s.codes.put(record).await {
        return oauth_error_to_http(OAuthError::Store(e.to_string()));
    }
    Json(AuthorizeResponse {
        code,
        state: req.state,
        redirect_uri: req.redirect_uri,
    })
    .into_response()
}

async fn post_oauth_token<R, S, P, C, K, A>(
    State(s): State<Arc<OAuthHttpState<R, S, P, C, K, A>>>,
    Json(req): Json<TokenExchangeRequest>,
) -> impl IntoResponse
where
    R: JwksResolver,
    S: PersonStore,
    P: ConsentPolicy,
    C: TimeSource,
    K: OAuthClientStore,
    A: AuthorizationCodeStore,
{
    let client = match s.clients.get(&req.client_id).await {
        Ok(Some(c)) => c,
        Ok(None) => return oauth_error_to_http(OAuthError::UnknownClient(req.client_id.clone())),
        Err(e) => return oauth_error_to_http(OAuthError::Store(e.to_string())),
    };
    if !client.is_enabled() {
        return oauth_error_to_http(OAuthError::ClientDisabled(req.client_id.clone()));
    }
    let record = match s.codes.take(&req.code).await {
        Ok(Some(r)) => r,
        Ok(None) => return oauth_error_to_http(OAuthError::CodeNotFound),
        Err(e) => return oauth_error_to_http(OAuthError::Store(e.to_string())),
    };
    let now = s.core.clock.now();
    if let Err(e) = validate_token_exchange(&req, &record, now) {
        return oauth_error_to_http(e);
    }
    match s
        .core
        .mint_oauth_auth(&record.client_id, &record.principal, &record.scope, s.core.auth_jwt_ttl_secs)
    {
        Ok(tok) => Json(OAuthTokenResponse {
            access_token: tok.auth_jwt,
            token_type: "Bearer".into(),
            expires_in: tok.expires_in,
            scope: tok.scope,
        })
        .into_response(),
        Err(e) => person_error_to_http(e),
    }
}

fn oauth_error_to_http(e: OAuthError) -> axum::response::Response {
    let code = match &e {
        OAuthError::UnsupportedResponseType(_)
        | OAuthError::UnsupportedGrantType(_)
        | OAuthError::UnsupportedPkceMethod(_)
        | OAuthError::RedirectUriMismatch(_)
        | OAuthError::CodeNotFound
        | OAuthError::CodeExpired
        | OAuthError::CodeClientMismatch
        | OAuthError::CodeRedirectMismatch
        | OAuthError::PkceVerificationFailed
        | OAuthError::ScopeNotAllowed => StatusCode::BAD_REQUEST,
        OAuthError::UnknownClient(_) | OAuthError::ClientDisabled(_) => StatusCode::UNAUTHORIZED,
        OAuthError::ConsentDenied(_) => StatusCode::FORBIDDEN,
        OAuthError::RequiresInteraction { url, code } => {
            let body = serde_json::json!({"requirement": "interaction", "url": url, "code": code});
            return (StatusCode::ACCEPTED, Json(body)).into_response();
        }
        OAuthError::Store(_) => StatusCode::INTERNAL_SERVER_ERROR,
    };
    (code, e.to_string()).into_response()
}

fn person_error_to_http(e: PersonError) -> axum::response::Response {
    let (code, body) = match &e {
        PersonError::BadRequest(_) => (StatusCode::BAD_REQUEST, e.to_string()),
        PersonError::ResourceTokenInvalid(_) => (StatusCode::UNAUTHORIZED, e.to_string()),
        PersonError::ConsentDenied(_) => (StatusCode::FORBIDDEN, e.to_string()),
        PersonError::RequiresInteraction { url, code } => {
            let body = serde_json::json!({"requirement": "interaction", "url": url, "code": code});
            return (StatusCode::ACCEPTED, Json(body)).into_response();
        }
        PersonError::Store(_) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
        PersonError::Encode(_) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    };
    (code, body).into_response()
}
