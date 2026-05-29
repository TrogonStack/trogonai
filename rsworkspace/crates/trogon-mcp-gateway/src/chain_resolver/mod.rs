//! Gateway-side act-chain registry resolution.
//!
//! Future ingress hook: in `gateway.rs`, immediately after JWT validation
//! (`resolve_with_claims`, ~line 255) and before CEL/authz (~line 345), call
//! `ChainResolver::resolve` on `jwt_claims.act_chain` using
//! `ChainResolutionMode::from_env()`.

mod cache;
mod errors;
mod mode;
mod registry_client;
mod resolver;

pub use errors::ChainResolutionError;
pub use mode::ChainResolutionMode;
pub use registry_client::{AgentRegistry, MockAgentRegistry, RegistryError, RegistryRecord};
pub use resolver::{ChainResolver, ResolvedChain};
