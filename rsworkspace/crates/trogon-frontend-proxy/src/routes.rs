//! Tenant → backend route table. Backed by a JetStream KV bucket in
//! production (`trogon-frontend-routes`); an in-memory store is
//! provided for tests.

use std::collections::HashMap;
use std::sync::Mutex;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

/// Name of the JetStream KV bucket that holds the route table.
pub const FRONTEND_ROUTES_BUCKET: &str = "trogon-frontend-routes";

/// HTTP request header carrying the tenant id. Clients (or an outer
/// auth layer) MUST set this; the proxy uses it as the routing key.
pub const TENANT_HEADER: &str = "x-trogon-tenant";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Backend {
    Mcp,
    A2a,
}

impl Backend {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Mcp => "mcp",
            Self::A2a => "a2a",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Route {
    pub tenant: String,
    pub backend: Backend,
    pub upstream_url: String,
    /// JWT audience to mint for the outbound mesh token.
    pub mesh_audience: String,
}

#[derive(Debug)]
pub enum RouteStoreError {
    Backend(String),
}

impl std::fmt::Display for RouteStoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Backend(message) => write!(f, "route store: {message}"),
        }
    }
}

impl std::error::Error for RouteStoreError {}

#[async_trait]
pub trait RouteStore: Send + Sync {
    async fn put(&self, route: Route) -> Result<(), RouteStoreError>;
    async fn get(&self, tenant: &str) -> Result<Option<Route>, RouteStoreError>;
}

#[derive(Default)]
pub struct InMemoryRouteStore {
    routes: Mutex<HashMap<String, Route>>,
}

impl InMemoryRouteStore {
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl RouteStore for InMemoryRouteStore {
    async fn put(&self, route: Route) -> Result<(), RouteStoreError> {
        let mut guard = self.routes.lock().expect("route store lock");
        guard.insert(route.tenant.clone(), route);
        Ok(())
    }

    async fn get(&self, tenant: &str) -> Result<Option<Route>, RouteStoreError> {
        let guard = self.routes.lock().expect("route store lock");
        Ok(guard.get(tenant).cloned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Route {
        Route {
            tenant: "acme".into(),
            backend: Backend::Mcp,
            upstream_url: "https://mcp.internal.example".into(),
            mesh_audience: "mesh.mcp".into(),
        }
    }

    #[tokio::test]
    async fn in_memory_round_trip() {
        let store = InMemoryRouteStore::new();
        store.put(sample()).await.unwrap();
        assert_eq!(store.get("acme").await.unwrap(), Some(sample()));
    }

    #[tokio::test]
    async fn missing_tenant_returns_none() {
        let store = InMemoryRouteStore::new();
        assert_eq!(store.get("nobody").await.unwrap(), None);
    }

    #[test]
    fn backend_str_is_stable() {
        assert_eq!(Backend::Mcp.as_str(), "mcp");
        assert_eq!(Backend::A2a.as_str(), "a2a");
    }

    #[test]
    fn route_serde_round_trip() {
        let json = serde_json::to_string(&sample()).unwrap();
        let back: Route = serde_json::from_str(&json).unwrap();
        assert_eq!(back, sample());
    }
}
