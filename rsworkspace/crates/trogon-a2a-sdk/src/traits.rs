use async_trait::async_trait;
use bytes::Bytes;

use crate::types::{AgentId, AgentRecord, ExchangeRequest, ExchangeResponse, SdkError};

#[async_trait]
pub trait SvidSource: Send + Sync {
    async fn current(&self) -> Result<String, SdkError>;
}

#[async_trait]
pub trait SubjectTokenSource: Send + Sync {
    async fn current(&self) -> Result<String, SdkError>;
}

#[async_trait]
pub trait Sts: Send + Sync {
    async fn exchange(&self, req: ExchangeRequest) -> Result<ExchangeResponse, SdkError>;
}

#[async_trait]
pub trait Registry: Send + Sync {
    async fn lookup(&self, agent_id: &AgentId) -> Result<AgentRecord, SdkError>;
}

#[async_trait]
pub trait MessageTransport: Send + Sync {
    async fn request(&self, subject: &str, payload: Bytes, headers: async_nats::HeaderMap) -> Result<Bytes, SdkError>;
}

#[async_trait]
pub trait Jwks: Send + Sync {
    async fn decoding_key(&self, kid: Option<&str>) -> Result<jsonwebtoken::DecodingKey, SdkError>;
}
