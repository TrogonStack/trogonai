//! Typed agent-to-agent SDK: registry lookup, STS exchange, and mesh-token verification.

pub mod client;
pub mod constants;
pub mod jwks;
pub mod registry;
pub mod server;
pub mod sts;
pub mod subject;
pub mod svid;
pub mod traits;
pub mod types;

pub use client::{Client, ClientBuilder};
pub use jwks::Hs256Jwks;
pub use server::{Handler, serve};
pub use traits::{Jwks, MessageTransport, Registry, Sts, SubjectTokenSource, SvidSource};
pub use types::{
    ActChainEntry, AgentId, AgentRecord, Audience, Caller, ExchangeRequest, ExchangeResponse, Purpose, SdkError,
};

#[cfg(feature = "nats")]
pub use registry::NatsRegistry;
#[cfg(feature = "nats")]
pub use sts::NatsSts;
pub use svid::FileSvidSource;
