//! CRD Rust types for Trogon and Gateway API resources watched by the controller.

mod gateway_api;
mod mcp_gateway_config;

pub use gateway_api::{
    Gateway, GatewayListener, GatewaySpec, HTTPBackendRef, HTTPRoute, HttpPathMatch,
    HttpRouteMatch, HttpRouteRule, HttpRouteSpec, RouteParentRef,
};
pub use mcp_gateway_config::{
    MCPGatewayConfig, MCPGatewayConfigCondition, MCPGatewayConfigSpec, MCPGatewayConfigStatus,
};
