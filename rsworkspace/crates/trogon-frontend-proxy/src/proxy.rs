//! HTTP reverse-proxy: resolve a route by tenant, mint a mesh JWT,
//! and forward to the upstream MCP/A2A backend.

use std::sync::Arc;

use async_trait::async_trait;
use axum::body::Body;
use axum::extract::State;
use axum::http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode, Uri};
use axum::response::{IntoResponse, Response};

use crate::audit::{FrontendAuditEvent, FrontendAuditOutcome};
use crate::routes::{Route, RouteStore, TENANT_HEADER};

/// Headers that must not be forwarded as-is on either leg. Authorization
/// is replaced with the mesh JWT; hop-by-hop headers are dropped.
const STRIPPED_REQUEST_HEADERS: &[&str] = &[
    "authorization",
    "connection",
    "keep-alive",
    "proxy-authenticate",
    "proxy-authorization",
    "te",
    "trailers",
    "transfer-encoding",
    "upgrade",
    "host",
];

const STRIPPED_RESPONSE_HEADERS: &[&str] = &[
    "connection",
    "keep-alive",
    "proxy-authenticate",
    "proxy-authorization",
    "te",
    "trailers",
    "transfer-encoding",
    "upgrade",
];

#[derive(Debug, thiserror::Error)]
pub enum ProxyError {
    #[error("tenant header `{0}` missing or malformed")]
    MissingTenantHeader(&'static str),
    #[error("no route registered for tenant `{0}`")]
    NoRoute(String),
    #[error("route store: {0}")]
    Store(String),
    #[error("token mint: {0}")]
    Mint(String),
    #[error("upstream request: {0}")]
    Upstream(String),
    #[error("upstream body read: {0}")]
    UpstreamBody(String),
    #[error("read request body: {0}")]
    RequestBody(String),
    #[error("invalid upstream url `{url}`: {message}")]
    InvalidUpstream { url: String, message: String },
}

#[async_trait]
pub trait TokenMinter: Send + Sync {
    /// Mint an outbound mesh JWT for the given upstream `audience`.
    async fn mint(&self, audience: &str, tenant: &str) -> Result<String, String>;
}

pub struct ProxyService<S, M>
where
    S: RouteStore,
    M: TokenMinter,
{
    pub routes: Arc<S>,
    pub minter: Arc<M>,
    pub http: reqwest::Client,
}

impl<S, M> ProxyService<S, M>
where
    S: RouteStore,
    M: TokenMinter,
{
    pub fn new(routes: Arc<S>, minter: Arc<M>, http: reqwest::Client) -> Self {
        Self { routes, minter, http }
    }

    pub fn resolve_tenant<'a>(&self, headers: &'a HeaderMap) -> Result<&'a str, ProxyError> {
        headers
            .get(TENANT_HEADER)
            .ok_or(ProxyError::MissingTenantHeader(TENANT_HEADER))
            .and_then(|v| {
                v.to_str()
                    .map_err(|_| ProxyError::MissingTenantHeader(TENANT_HEADER))
            })
            .map(|s| {
                if s.is_empty() {
                    Err(ProxyError::MissingTenantHeader(TENANT_HEADER))
                } else {
                    Ok(s)
                }
            })
            .and_then(|r| r)
    }

    pub async fn route_for(&self, tenant: &str) -> Result<Route, ProxyError> {
        match self.routes.get(tenant).await {
            Ok(Some(r)) => Ok(r),
            Ok(None) => Err(ProxyError::NoRoute(tenant.to_string())),
            Err(e) => Err(ProxyError::Store(e.to_string())),
        }
    }
}

/// Axum handler type alias to make the wiring on the bin side terser.
pub type ProxyHandler<S, M> = Arc<ProxyService<S, M>>;

pub async fn dispatch<S, M>(
    State(service): State<ProxyHandler<S, M>>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Body,
) -> Response
where
    S: RouteStore + 'static,
    M: TokenMinter + 'static,
{
    let tenant = match service.resolve_tenant(&headers) {
        Ok(t) => t.to_string(),
        Err(e) => return error_response(StatusCode::BAD_REQUEST, e),
    };

    let route = match service.route_for(&tenant).await {
        Ok(r) => r,
        Err(e @ ProxyError::NoRoute(_)) => return error_response(StatusCode::NOT_FOUND, e),
        Err(e) => return error_response(StatusCode::INTERNAL_SERVER_ERROR, e),
    };

    let mesh_token = match route.token_exchange_mode {
        crate::routes::TokenExchangeMode::AuthOnly => None,
        _ => match service.minter.mint(&route.mesh_audience, &tenant).await {
            Ok(t) => Some(t),
            Err(e) => return error_response(StatusCode::BAD_GATEWAY, ProxyError::Mint(e)),
        },
    };

    let upstream = match build_upstream_url(&route.upstream_url, &uri) {
        Ok(u) => u,
        Err(e) => return error_response(StatusCode::BAD_GATEWAY, e),
    };

    let body_bytes = match axum::body::to_bytes(body, usize::MAX).await {
        Ok(b) => b,
        Err(e) => return error_response(StatusCode::BAD_REQUEST, ProxyError::RequestBody(e.to_string())),
    };

    let mut req = service.http.request(reqwest_method(&method), upstream);
    if let Some(token) = mesh_token.as_deref() {
        req = req.header("authorization", format!("Bearer {token}"));
    }

    for (name, value) in headers.iter() {
        if STRIPPED_REQUEST_HEADERS.iter().any(|h| name.as_str().eq_ignore_ascii_case(h)) {
            continue;
        }
        req = req.header(name.as_str(), value.as_bytes());
    }

    if !body_bytes.is_empty() {
        req = req.body(body_bytes);
    }

    let upstream_response = match req.send().await {
        Ok(r) => r,
        Err(e) => return error_response(StatusCode::BAD_GATEWAY, ProxyError::Upstream(e.to_string())),
    };

    relay_response(upstream_response).await
}

async fn relay_response(upstream: reqwest::Response) -> Response {
    let status = StatusCode::from_u16(upstream.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
    let mut headers = HeaderMap::new();
    for (name, value) in upstream.headers().iter() {
        if STRIPPED_RESPONSE_HEADERS
            .iter()
            .any(|h| name.as_str().eq_ignore_ascii_case(h))
        {
            continue;
        }
        if let (Ok(n), Ok(v)) = (
            HeaderName::from_bytes(name.as_str().as_bytes()),
            HeaderValue::from_bytes(value.as_bytes()),
        ) {
            headers.insert(n, v);
        }
    }
    let bytes = match upstream.bytes().await {
        Ok(b) => b,
        Err(e) => {
            return error_response(StatusCode::BAD_GATEWAY, ProxyError::UpstreamBody(e.to_string()));
        }
    };
    (status, headers, bytes).into_response()
}

fn error_response(status: StatusCode, error: ProxyError) -> Response {
    (status, error.to_string()).into_response()
}

fn reqwest_method(method: &Method) -> reqwest::Method {
    reqwest::Method::from_bytes(method.as_str().as_bytes()).unwrap_or(reqwest::Method::POST)
}

pub fn build_upstream_url(upstream_base: &str, request_uri: &Uri) -> Result<String, ProxyError> {
    let base = upstream_base.trim_end_matches('/');
    let path = request_uri.path();
    let suffix = request_uri.query().map(|q| format!("?{q}")).unwrap_or_default();
    if !path.starts_with('/') {
        return Err(ProxyError::InvalidUpstream {
            url: upstream_base.to_string(),
            message: "request path missing leading slash".into(),
        });
    }
    Ok(format!("{base}{path}{suffix}"))
}

/// Build a [`FrontendAuditEvent`] without an upstream status (caller fills it in).
pub fn audit_for(request_id: &str, tenant: &str, route: &Route, method: &Method, uri: &Uri) -> FrontendAuditEvent {
    FrontendAuditEvent {
        request_id: request_id.to_string(),
        tenant: tenant.to_string(),
        backend: route.backend,
        upstream_url: route.upstream_url.clone(),
        method: method.as_str().to_string(),
        path: uri.path().to_string(),
        outcome: FrontendAuditOutcome::Dispatched,
        upstream_status: None,
        error: None,
        traceparent: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::routes::{Backend, InMemoryRouteStore, Route};

    struct StaticMinter;

    #[async_trait]
    impl TokenMinter for StaticMinter {
        async fn mint(&self, audience: &str, tenant: &str) -> Result<String, String> {
            Ok(format!("mesh::{audience}::{tenant}"))
        }
    }

    fn service() -> ProxyService<InMemoryRouteStore, StaticMinter> {
        ProxyService::new(
            Arc::new(InMemoryRouteStore::new()),
            Arc::new(StaticMinter),
            reqwest::Client::new(),
        )
    }

    fn route() -> Route {
        Route {
            tenant: "acme".into(),
            backend: Backend::Mcp,
            upstream_url: "https://mcp.internal.example".into(),
            mesh_audience: "mesh.mcp".into(),
            token_exchange_mode: crate::routes::TokenExchangeMode::default(),
        }
    }

    #[tokio::test]
    async fn resolve_tenant_reads_header() {
        let svc = service();
        let mut headers = HeaderMap::new();
        headers.insert(TENANT_HEADER, HeaderValue::from_static("acme"));
        assert_eq!(svc.resolve_tenant(&headers).unwrap(), "acme");
    }

    #[tokio::test]
    async fn resolve_tenant_rejects_missing_header() {
        let svc = service();
        let headers = HeaderMap::new();
        let err = svc.resolve_tenant(&headers).unwrap_err();
        assert!(matches!(err, ProxyError::MissingTenantHeader(_)));
    }

    #[tokio::test]
    async fn resolve_tenant_rejects_empty_header() {
        let svc = service();
        let mut headers = HeaderMap::new();
        headers.insert(TENANT_HEADER, HeaderValue::from_static(""));
        let err = svc.resolve_tenant(&headers).unwrap_err();
        assert!(matches!(err, ProxyError::MissingTenantHeader(_)));
    }

    #[tokio::test]
    async fn unknown_tenant_yields_no_route() {
        let svc = service();
        let err = svc.route_for("unknown").await.unwrap_err();
        assert!(matches!(err, ProxyError::NoRoute(_)));
    }

    #[tokio::test]
    async fn known_tenant_returns_route() {
        let svc = service();
        svc.routes.put(route()).await.unwrap();
        let got = svc.route_for("acme").await.unwrap();
        assert_eq!(got, route());
    }

    #[test]
    fn upstream_url_joins_base_and_request_path() {
        let uri: Uri = "/messages?foo=1".parse().unwrap();
        assert_eq!(
            build_upstream_url("https://mcp.internal.example/", &uri).unwrap(),
            "https://mcp.internal.example/messages?foo=1"
        );
    }

    #[test]
    fn upstream_url_handles_base_without_trailing_slash() {
        let uri: Uri = "/messages".parse().unwrap();
        assert_eq!(
            build_upstream_url("https://mcp.internal.example", &uri).unwrap(),
            "https://mcp.internal.example/messages"
        );
    }

    #[test]
    fn audit_event_carries_route_and_request_metadata() {
        let event = audit_for("req-1", "acme", &route(), &Method::POST, &"/m?a=1".parse().unwrap());
        assert_eq!(event.tenant, "acme");
        assert_eq!(event.backend, Backend::Mcp);
        assert_eq!(event.upstream_url, "https://mcp.internal.example");
        assert_eq!(event.method, "POST");
        assert_eq!(event.path, "/m");
        assert_eq!(event.outcome, FrontendAuditOutcome::Dispatched);
    }

    #[tokio::test]
    async fn auth_only_route_does_not_mint_mesh_token() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};
        let upstream = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/m"))
            .respond_with(ResponseTemplate::new(200))
            .expect(1)
            .mount(&upstream)
            .await;
        let store = InMemoryRouteStore::new();
        store
            .put(Route {
                tenant: "acme".into(),
                backend: Backend::Mcp,
                upstream_url: upstream.uri(),
                mesh_audience: "mesh.mcp".into(),
                token_exchange_mode: crate::routes::TokenExchangeMode::AuthOnly,
            })
            .await
            .unwrap();
        struct FailingMinter;
        #[async_trait]
        impl TokenMinter for FailingMinter {
            async fn mint(&self, _audience: &str, _tenant: &str) -> Result<String, String> {
                panic!("mint must not be called in AuthOnly mode");
            }
        }
        let svc = Arc::new(ProxyService::new(
            Arc::new(store),
            Arc::new(FailingMinter),
            reqwest::Client::new(),
        ));
        let mut headers = HeaderMap::new();
        headers.insert(TENANT_HEADER, HeaderValue::from_static("acme"));
        let resp = dispatch(
            axum::extract::State(svc),
            Method::GET,
            "/m".parse().unwrap(),
            headers,
            Body::empty(),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
    }
}
