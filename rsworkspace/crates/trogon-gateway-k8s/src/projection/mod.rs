//! Pure CRD → KV value translation (no I/O).

mod gateway_api;
mod gateway_config;
mod keys;

pub use gateway_api::{GatewayKvValue, HttpRouteKvValue, project_gateway, project_http_route};
pub use gateway_config::{
    ActiveBundlePointer, GatewayConfigKvValue, ProjectionError, build_status, content_hash_hex, decode_gateway_config,
    encode_gateway_config, project_mcp_gateway_config,
};
pub use keys::{gateway_kv_key, gateway_route_kv_key, http_route_kv_key, mcp_gateway_config_kv_key};
