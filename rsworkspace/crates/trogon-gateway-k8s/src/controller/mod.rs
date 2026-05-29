//! `kube::runtime::controller::Controller` reconcilers per watched CRD kind.

mod gateway;
mod http_route;
mod mcp_gateway_config;
pub mod policy;

pub use gateway::run_gateway_controller;
pub use http_route::run_http_route_controller;
pub use mcp_gateway_config::run_mcp_gateway_config_controller;

use std::sync::Arc;

use kube::Client;

use crate::nats::ConfigKv;

pub struct ControllerContext {
    pub client: Client,
    pub kv: Arc<dyn ConfigKv>,
}
