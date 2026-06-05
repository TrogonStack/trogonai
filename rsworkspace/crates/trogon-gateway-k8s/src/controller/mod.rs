//! `kube::runtime::controller::Controller` reconcilers per watched CRD kind.

mod agentgateway_backend;
mod enterprise_policy;
mod gateway;
mod http_route;
mod mcp_gateway_config;
pub mod policy;

pub use agentgateway_backend::run_agentgateway_backend_controller;
pub use enterprise_policy::run_enterprise_policy_controller;
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
