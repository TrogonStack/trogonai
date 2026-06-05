//! CRD Rust types for Trogon and Gateway API resources watched by the controller.

mod agentgateway_backend;
mod enterprise_policy;
mod gateway_api;
mod mcp_gateway_config;

pub use agentgateway_backend::{
    AgentgatewayBackend, AgentgatewayBackendSpec, AgentgatewayBackendStatus, BackendCondition,
    BackendTarget, JwtValidation, ISSUER_PROXY_ANNOTATION,
};
pub use enterprise_policy::{
    BackendPolicy, EnterpriseAgentgatewayPolicy, EnterpriseAgentgatewayPolicyCondition,
    EnterpriseAgentgatewayPolicySpec, EnterpriseAgentgatewayPolicyStatus, EntraConfig, JwksSource,
    JwtAuthentication, JwtProvider, PolicyTargetRef, SecretKeyRef, TokenExchangePolicy, TrafficPolicy,
};
pub use gateway_api::{
    Gateway, GatewayListener, GatewaySpec, HTTPBackendRef, HTTPRoute, HttpPathMatch, HttpRouteMatch, HttpRouteRule,
    HttpRouteSpec, RouteParentRef,
};
pub use mcp_gateway_config::{
    MCPGatewayConfig, MCPGatewayConfigCondition, MCPGatewayConfigSpec, MCPGatewayConfigStatus,
};
