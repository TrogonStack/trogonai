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
use crate::policy::ConsentPolicy;
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
