//! NATS request/reply client for `mcp.sts.exchange`.

mod client;
mod config;
mod error;

pub use client::{MintedMeshToken, StsClient, build_exchange_request};
pub use config::StsClientConfig;
pub use error::StsClientError;
pub use trogon_sts::{EXCHANGE_SUBJECT, StsExchangeRequest, StsExchangeResponse, StsTokenErrorResponse};
