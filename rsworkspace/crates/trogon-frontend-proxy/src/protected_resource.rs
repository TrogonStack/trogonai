//! Per-backend OAuth Protected Resource Metadata (RFC 9728).
//!
//! Solo.io's agentgateway consent-screen spec requires that MCP
//! backends expose `/.well-known/oauth-protected-resource/mcp/{backend}`
//! so clients can discover which authorization server protects each
//! backend. The companion path
//! `/.well-known/oauth-authorization-server/mcp/{backend}` re-publishes
//! the RFC 8414 authorization-server doc per-backend.
//!
//! Both routes are read-only and cheap to serve from gateway state.

use std::sync::Arc;

use axum::Json;
use axum::Router;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProtectedResourceMetadata {
    pub resource: String,
    pub authorization_servers: Vec<String>,
    pub bearer_methods_supported: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub scopes_supported: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource_documentation: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthorizationServerMetadata {
    pub issuer: String,
    pub authorization_endpoint: String,
    pub token_endpoint: String,
    pub jwks_uri: String,
    pub response_types_supported: Vec<String>,
    pub grant_types_supported: Vec<String>,
    pub code_challenge_methods_supported: Vec<String>,
    pub token_endpoint_auth_methods_supported: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct GatewayDiscovery {
    pub base_url: String,
    pub issuer: String,
    pub authorization_endpoint: String,
    pub token_endpoint: String,
    pub jwks_uri: String,
}

impl GatewayDiscovery {
    pub fn protected_resource(&self, backend: &str) -> ProtectedResourceMetadata {
        ProtectedResourceMetadata {
            resource: format!("{}/mcp/{}", self.base_url.trim_end_matches('/'), backend),
            authorization_servers: vec![self.issuer.clone()],
            bearer_methods_supported: vec!["header".into()],
            scopes_supported: Vec::new(),
            resource_documentation: None,
        }
    }

    pub fn authorization_server(&self) -> AuthorizationServerMetadata {
        AuthorizationServerMetadata {
            issuer: self.issuer.clone(),
            authorization_endpoint: self.authorization_endpoint.clone(),
            token_endpoint: self.token_endpoint.clone(),
            jwks_uri: self.jwks_uri.clone(),
            response_types_supported: vec!["code".into()],
            grant_types_supported: vec![
                "authorization_code".into(),
                "refresh_token".into(),
                "urn:ietf:params:oauth:grant-type:token-exchange".into(),
                "urn:ietf:params:oauth:grant-type:jwt-bearer".into(),
            ],
            code_challenge_methods_supported: vec!["S256".into()],
            token_endpoint_auth_methods_supported: vec!["client_secret_basic".into(), "none".into()],
        }
    }
}

pub fn router(discovery: Arc<GatewayDiscovery>) -> Router {
    Router::new()
        .route(
            "/.well-known/oauth-protected-resource/mcp/{backend}",
            get(get_protected_resource),
        )
        .route(
            "/.well-known/oauth-authorization-server/mcp/{backend}",
            get(get_authorization_server),
        )
        .with_state(discovery)
}

async fn get_protected_resource(State(d): State<Arc<GatewayDiscovery>>, Path(backend): Path<String>) -> Response {
    if backend.is_empty() || backend.contains('/') {
        return (StatusCode::BAD_REQUEST, "invalid backend").into_response();
    }
    Json(d.protected_resource(&backend)).into_response()
}

async fn get_authorization_server(State(d): State<Arc<GatewayDiscovery>>, Path(backend): Path<String>) -> Response {
    if backend.is_empty() || backend.contains('/') {
        return (StatusCode::BAD_REQUEST, "invalid backend").into_response();
    }
    Json(d.authorization_server()).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    fn discovery() -> Arc<GatewayDiscovery> {
        Arc::new(GatewayDiscovery {
            base_url: "https://gw.example.com".into(),
            issuer: "https://gw.example.com/aauth".into(),
            authorization_endpoint: "https://gw.example.com/aauth/oauth/authorize".into(),
            token_endpoint: "https://gw.example.com/aauth/oauth/token".into(),
            jwks_uri: "https://gw.example.com/aauth/.well-known/jwks.json".into(),
        })
    }

    #[tokio::test]
    async fn protected_resource_doc_serves_per_backend_resource_uri() {
        let app = router(discovery());
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/.well-known/oauth-protected-resource/mcp/fileserver")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let doc: ProtectedResourceMetadata = serde_json::from_slice(&body).unwrap();
        assert_eq!(doc.resource, "https://gw.example.com/mcp/fileserver");
        assert_eq!(doc.authorization_servers, vec!["https://gw.example.com/aauth"]);
        assert!(doc.bearer_methods_supported.contains(&"header".to_string()));
    }

    #[tokio::test]
    async fn router_merged_with_catchall_still_resolves_well_known() {
        use axum::routing::any;

        async fn catchall() -> &'static str {
            "catchall"
        }

        let app = router(discovery()).merge(Router::new().route("/{*path}", any(catchall)));

        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/.well-known/oauth-protected-resource/mcp/fileserver")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let doc: ProtectedResourceMetadata = serde_json::from_slice(&body).unwrap();
        assert_eq!(doc.resource, "https://gw.example.com/mcp/fileserver");

        let fall_through = app
            .oneshot(Request::builder().uri("/some/tenant/path").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(fall_through.status(), StatusCode::OK);
        let body = axum::body::to_bytes(fall_through.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(&body[..], b"catchall");
    }

    #[tokio::test]
    async fn authorization_server_doc_per_backend_advertises_grant_types() {
        let app = router(discovery());
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/.well-known/oauth-authorization-server/mcp/search")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let doc: AuthorizationServerMetadata = serde_json::from_slice(&body).unwrap();
        assert!(doc.grant_types_supported.iter().any(|g| g == "authorization_code"));
        assert!(
            doc.grant_types_supported
                .iter()
                .any(|g| g == "urn:ietf:params:oauth:grant-type:token-exchange")
        );
        assert_eq!(doc.code_challenge_methods_supported, vec!["S256"]);
    }
}
