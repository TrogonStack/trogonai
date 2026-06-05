//! Frontend HTTP reverse-proxy in front of MCP and A2A gateways.
//!
//! Per `GCP_TODO.md §3.7`: external HTTPS clients hit this proxy,
//! which resolves a route per tenant and forwards to the right
//! backend (MCP or A2A) carrying a mesh JWT minted by `trogon-sts`.
//! No bare passthrough — every hop is authenticated.
//!
//! This crate ships the routing + dispatch primitives. TLS
//! termination reuses the rustls plumbing shipped for workstream
//! 3.1 (`trogon-mcp-gateway::tls`) so we don't redo cert reload.

pub mod audit;
pub mod protected_resource;
pub mod proxy;
pub mod routes;

pub use audit::{FrontendAuditEvent, FrontendAuditOutcome, audit_subject};
pub use proxy::{ProxyError, ProxyHandler, ProxyService, TokenMinter};
pub use routes::{
    Backend, FRONTEND_ROUTES_BUCKET, InMemoryRouteStore, Route, RouteStore, RouteStoreError,
    TENANT_HEADER,
};
