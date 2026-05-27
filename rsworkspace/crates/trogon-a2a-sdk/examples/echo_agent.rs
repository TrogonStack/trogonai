//! Echo agent — returns inbound payload unchanged.

use async_trait::async_trait;
use bytes::Bytes;
use trogon_a2a_sdk::{AgentId, Handler, Hs256Jwks, SdkError, serve};

struct Echo;

#[async_trait]
impl Handler for Echo {
    async fn handle(&self, _caller: trogon_a2a_sdk::Caller, raw_payload: Bytes) -> Result<Bytes, SdkError> {
        Ok(raw_payload)
    }
}

#[tokio::main]
async fn main() -> Result<(), SdkError> {
    let nats = async_nats::connect(std::env::var("NATS_URL").unwrap_or_else(|_| "nats://127.0.0.1:4222".into()))
        .await
        .map_err(SdkError::nats)?;
    let own = AgentId::parse(std::env::var("AGENT_ID").unwrap_or_else(|_| "acme/echo-agent".into()))
        .map_err(|e| SdkError::Config(e.to_string()))?;
    let secret = std::env::var("MESH_JWT_HS256_SECRET").unwrap_or_else(|_| "dev-only-change-me".into());
    serve(nats, own, Hs256Jwks::new(secret.into_bytes()), Echo).await
}
