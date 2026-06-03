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
