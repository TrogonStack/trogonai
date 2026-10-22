use crate::{AuthError, Authenticator, HttpTransportConfig, PostgresMcp, QueryService};
use axum::{
    Json, Router,
    extract::{Request, State},
    http::{HeaderName, HeaderValue, Method, StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::get,
};
use rmcp::transport::streamable_http_server::{
    StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
};
use serde::Serialize;
use std::sync::Arc;
use tower_http::cors::{AllowOrigin, CorsLayer};

#[derive(Clone)]
struct Admission {
    authenticator: Arc<Authenticator>,
    challenge: HeaderValue,
    insufficient_scope_challenge: HeaderValue,
}

#[derive(Clone, Serialize)]
struct ProtectedResourceMetadata {
    resource: String,
    authorization_servers: Vec<String>,
    bearer_methods_supported: Vec<String>,
}

pub fn http_router(service: QueryService, authenticator: Arc<Authenticator>, config: HttpTransportConfig) -> Router {
    let origins = config.allowed_origins().to_vec();
    let cors = CorsLayer::new()
        .allow_origin(AllowOrigin::predicate(move |origin, _| {
            origins.iter().any(|allowed| origin.as_bytes() == allowed.as_bytes())
        }))
        .allow_methods([Method::GET, Method::POST, Method::DELETE])
        .allow_headers([
            header::AUTHORIZATION,
            header::CONTENT_TYPE,
            header::ACCEPT,
            HeaderName::from_static("mcp-protocol-version"),
        ])
        .expose_headers([header::WWW_AUTHENTICATE]);
    let mcp = PostgresMcp::new(service);
    let http_config = StreamableHttpServerConfig::default()
        .with_legacy_session_mode(false)
        .with_json_response(true)
        .with_allowed_hosts(vec![config.authority()])
        .with_allowed_origins(config.transport_origins().to_vec())
        .enforce_origin_validation()
        .with_max_request_body_bytes(crate::constants::MCP_MAX_REQUEST_BODY_BYTES);
    let transport: StreamableHttpService<PostgresMcp, LocalSessionManager> =
        StreamableHttpService::new(move || Ok(mcp.clone()), Default::default(), http_config);
    let metadata = ProtectedResourceMetadata {
        resource: config.resource_url().to_owned(),
        authorization_servers: authenticator.issuer_urls(),
        bearer_methods_supported: vec!["header".into()],
    };
    let challenge = HeaderValue::from_str(&format!("Bearer resource_metadata=\"{}\"", config.metadata_url()))
        .unwrap_or_else(|_| HeaderValue::from_static("Bearer"));
    let insufficient_scope_challenge = HeaderValue::from_str(&format!(
        "Bearer error=\"insufficient_scope\", resource_metadata=\"{}\"",
        config.metadata_url()
    ))
    .unwrap_or_else(|_| HeaderValue::from_static("Bearer error=\"insufficient_scope\""));
    let admission = Admission {
        authenticator,
        challenge,
        insufficient_scope_challenge,
    };
    Router::new()
        .route_service("/mcp", transport)
        .route_layer(middleware::from_fn_with_state(admission, authenticate))
        .route(
            "/.well-known/oauth-protected-resource/mcp",
            get(move || async move { Json(metadata) }),
        )
        .layer(cors)
}

async fn authenticate(State(admission): State<Admission>, mut request: Request, next: Next) -> Response {
    let token = bearer_token(request.headers());
    let Some(token) = token else {
        return rejected(&admission, StatusCode::UNAUTHORIZED);
    };
    match admission.authenticator.authenticate(token).await {
        Ok(identity) => {
            request.extensions_mut().insert(identity);
            next.run(request).await
        }
        Err(AuthError::InsufficientScope) => rejected(&admission, StatusCode::FORBIDDEN),
        Err(AuthError::Unavailable) => rejected(&admission, StatusCode::SERVICE_UNAVAILABLE),
        Err(AuthError::Configuration | AuthError::Unauthenticated) => rejected(&admission, StatusCode::UNAUTHORIZED),
    }
}

fn bearer_token(headers: &axum::http::HeaderMap) -> Option<&str> {
    let mut values = headers.get_all(header::AUTHORIZATION).iter();
    let value = values.next()?.to_str().ok()?;
    if values.next().is_some() {
        return None;
    }
    let (scheme, token) = value.split_once(' ')?;
    if !scheme.eq_ignore_ascii_case("Bearer")
        || token.is_empty()
        || token.bytes().any(|byte| byte.is_ascii_whitespace())
    {
        return None;
    }
    Some(token)
}

fn rejected(admission: &Admission, status: StatusCode) -> Response {
    let mut response = status.into_response();
    if matches!(status, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN) {
        response.headers_mut().insert(
            header::WWW_AUTHENTICATE,
            if status == StatusCode::FORBIDDEN {
                admission.insufficient_scope_challenge.clone()
            } else {
                admission.challenge.clone()
            },
        );
    }
    response
}
