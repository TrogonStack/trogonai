//! Pure CRD → KV value translation (no I/O).

mod backend;
mod gateway_api;
mod gateway_config;
mod keys;
mod policy;

pub use backend::{
    AgentgatewayBackendKvValue, AgentgatewayBackendProjection, BackendTargetKv, JwtValidationKv,
    agentgateway_backend_kv_key, build_agentgateway_backend_status, project_agentgateway_backend,
};
pub use gateway_api::{GatewayKvValue, HttpRouteKvValue, project_gateway, project_http_route};
pub use gateway_config::{
    ActiveBundlePointer, GatewayConfigKvValue, ProjectionError, build_status, content_hash_hex, decode_gateway_config,
    encode_gateway_config, project_mcp_gateway_config,
};
pub use keys::{gateway_kv_key, gateway_route_kv_key, http_route_kv_key, mcp_gateway_config_kv_key};
pub use policy::{
    EnterprisePolicyKvValue, EnterprisePolicyProjection, EntraKv, JwtAuthenticationKv, JwtProviderKv, PolicyTargetKv,
    TokenExchangeKv, build_enterprise_policy_status, enterprise_policy_kv_key, project_enterprise_policy,
};
