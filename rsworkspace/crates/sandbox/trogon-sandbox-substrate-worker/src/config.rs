//! Worker configuration, shared by the binary and the tests.

mod constants;
mod router_endpoint;
mod router_endpoint_error;
mod sandbox_class;
mod sandbox_class_error;
mod substrate_config;
mod substrate_config_error;

pub use constants::{DEFAULT_ROUTER_ENDPOINT, SANDBOX_CLASS_LABEL, SIZE_CLASS_LABEL};
pub use router_endpoint::RouterEndpoint;
pub use router_endpoint_error::RouterEndpointError;
pub use sandbox_class::SandboxClass;
pub use sandbox_class_error::SandboxClassError;
pub use substrate_config::SubstrateConfig;
pub use substrate_config_error::SubstrateConfigError;

#[cfg(test)]
mod tests;
