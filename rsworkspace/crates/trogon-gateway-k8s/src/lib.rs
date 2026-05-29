//! Kubernetes controller that projects Gateway API and Trogon CRDs into `mcp-gateway-config` KV.

pub mod controller;
pub mod crd;
pub mod health;
pub mod nats;
pub mod projection;

pub const DEFAULT_CONFIG_BUCKET: &str = "mcp-gateway-config";
