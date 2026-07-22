use std::fmt;
use std::sync::Arc;

use a2a_nats::catalog::RegistrarSubject;
use a2a_nats::{A2aAgentId, A2aPrefix};
use async_trait::async_trait;
use bytes::Bytes;

use crate::error::BridgeError;
use crate::identity::BridgeAgentId;

#[async_trait]
pub trait OutboundHttpsAgentUpstream: Send + Sync {
    async fn proxy_jsonrpc_post(
        &self,
        agent_id: AgentRegistrationId,
        method: MethodSegment,
        body: Bytes,
    ) -> Result<Bytes, BridgeError>;
}

#[derive(Clone, Copy, Debug)]
pub struct StubOutboundForwarder;

#[async_trait]
impl OutboundHttpsAgentUpstream for StubOutboundForwarder {
    async fn proxy_jsonrpc_post(
        &self,
        agent_id: AgentRegistrationId,
        method: MethodSegment,
        body: Bytes,
    ) -> Result<Bytes, BridgeError> {
        let _: (_, _, _) = (agent_id, method, body.len());
        Err(BridgeError::UpstreamHttps(
            "HTTPS upstream forwarder not wired for this runtime".into(),
        ))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct AgentRegistrationId(String);

impl AgentRegistrationId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for AgentRegistrationId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.pad(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct MethodSegment(String);

impl MethodSegment {
    pub fn new(segment: impl Into<String>) -> Self {
        Self(segment.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for MethodSegment {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.pad(self.as_str())
    }
}

#[async_trait]
pub trait JsonHttpPost: Send + Sync {
    async fn post_application_json(&self, url: &str, body: &[u8]) -> Result<Vec<u8>, BridgeError>;
}

#[async_trait]
pub trait OutboundUpstreamUrlResolve: Send + Sync {
    async fn downstream_post_root(&self, agent_id: &AgentRegistrationId) -> Result<String, BridgeError>;
}

pub struct ReqwestJsonHttpPoster {
    client: reqwest::Client,
}

impl ReqwestJsonHttpPoster {
    #[must_use]
    pub fn new(client: reqwest::Client) -> Self {
        Self { client }
    }

    #[must_use = "poster must be wired into upstream before use"]
    pub fn default_https_client() -> Result<Self, BridgeError> {
        reqwest::Client::builder()
            .build()
            .map(Self::new)
            .map_err(|e| BridgeError::UpstreamHttps(e.to_string()))
    }
}

#[async_trait]
impl JsonHttpPost for ReqwestJsonHttpPoster {
    async fn post_application_json(&self, url: &str, body: &[u8]) -> Result<Vec<u8>, BridgeError> {
        let response = self
            .client
            .post(url)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(body.to_vec())
            .send()
            .await
            .map_err(|e| BridgeError::UpstreamHttps(e.to_string()))?;
        // A 4xx/5xx body is not a valid JSON-RPC reply — forwarding it
        // would let callers misread an upstream failure as a successful
        // agent response. Capture status before consuming the body so
        // the error message is actionable.
        let status = response.status();
        let bytes = response
            .bytes()
            .await
            .map_err(|e| BridgeError::UpstreamHttps(e.to_string()))?;
        if !status.is_success() {
            return Err(BridgeError::UpstreamHttps(format!(
                "upstream HTTPS returned {status}: {}",
                String::from_utf8_lossy(&bytes)
            )));
        }
        Ok(bytes.to_vec())
    }
}

#[derive(Clone)]
pub struct MappedHttpsUpstream<H, U> {
    http: Arc<H>,
    resolve: Arc<U>,
}

impl<H: JsonHttpPost, U: OutboundUpstreamUrlResolve> MappedHttpsUpstream<H, U> {
    pub fn new(http: Arc<H>, resolve: Arc<U>) -> Self {
        Self { http, resolve }
    }
}

#[async_trait]
impl<H: JsonHttpPost + Send + Sync, U: OutboundUpstreamUrlResolve> OutboundHttpsAgentUpstream
    for MappedHttpsUpstream<H, U>
{
    async fn proxy_jsonrpc_post(
        &self,
        agent_id: AgentRegistrationId,
        _method: MethodSegment,
        body: Bytes,
    ) -> Result<Bytes, BridgeError> {
        let url = self.resolve.downstream_post_root(&agent_id).await?;
        let bytes = self.http.post_application_json(&url, &body).await?;
        Ok(Bytes::from(bytes))
    }
}

#[async_trait]
pub trait CatalogRegistrationPublish: Send + Sync {
    async fn publish_core(&self, subject: impl AsRef<str> + Send, payload: &[u8]) -> Result<(), BridgeError>;
}

pub async fn publish_https_agent_card_to_catalog<R: CatalogRegistrationPublish + ?Sized>(
    registrar: &R,
    prefix: &A2aPrefix,
    agent_id: &A2aAgentId,
    card_json: &[u8],
) -> Result<(), BridgeError> {
    let subject = RegistrarSubject::new(prefix).for_agent(agent_id);
    registrar.publish_core(subject.as_str(), card_json).await?;

    Ok(())
}

pub async fn publish_https_agent_card_registered<R: CatalogRegistrationPublish + ?Sized>(
    registrar: &R,
    prefix: &A2aPrefix,
    agent_id: &BridgeAgentId,
    card_json: &[u8],
) -> Result<(), BridgeError> {
    publish_https_agent_card_to_catalog(registrar, prefix, agent_id.as_agent_id(), card_json).await
}

pub struct NatsCoreCatalogRegistrar {
    client: Arc<async_nats::Client>,
}

impl NatsCoreCatalogRegistrar {
    #[must_use]
    pub fn new(client: Arc<async_nats::Client>) -> Self {
        Self { client }
    }
}

#[async_trait]
impl CatalogRegistrationPublish for NatsCoreCatalogRegistrar {
    async fn publish_core(&self, subject: impl AsRef<str> + Send, payload: &[u8]) -> Result<(), BridgeError> {
        self.client
            .publish(subject.as_ref().to_owned(), Bytes::copy_from_slice(payload))
            .await
            .map_err(|e| BridgeError::CatalogRegistration(e.to_string()))
    }
}

pub async fn forward<U: OutboundHttpsAgentUpstream + ?Sized>(
    upstream: &U,
    agent_id: AgentRegistrationId,
    method: MethodSegment,
    body: Bytes,
) -> Result<Bytes, BridgeError> {
    upstream.proxy_jsonrpc_post(agent_id, method, body).await
}

#[cfg(test)]
mod tests;
