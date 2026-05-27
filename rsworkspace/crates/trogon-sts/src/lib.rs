pub mod audit;
pub mod cache;
pub mod error;
pub mod exchange;
pub mod limits;
pub mod registry;
pub mod signer;
pub mod token_verify;
pub mod trust;
pub mod types;

pub use exchange::{ExchangeRequest, ExchangeService, ExchangeSuccess};
pub use types::{StsExchangeRequest, StsExchangeResponse, StsTokenErrorResponse};

pub const DEFAULT_MESH_ISSUER: &str = "urn:trogon:sts:mesh";
pub const DEFAULT_MESH_TOKEN_TTL_SECS: u64 = 120;
pub const DEFAULT_REGISTRY_SUBJECT: &str = "mcp.registry.agent.lookup";
pub const DEFAULT_QUEUE_GROUP: &str = "trogon-sts";
pub const EXCHANGE_SUBJECT: &str = "mcp.sts.exchange";
pub const JWKS_KV_BUCKET: &str = "mcp-jwks";
pub const JWKS_KV_MESH_KEY: &str = "mesh/current";
pub const MIN_MESH_TOKEN_TTL_SECS: u64 = 60;
pub const MAX_MESH_TOKEN_TTL_SECS: u64 = 300;
pub const CLOCK_SKEW_SECS: i64 = 30;
