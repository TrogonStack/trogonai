use std::fmt;
use std::sync::Arc;

use async_trait::async_trait;
use bytes::Bytes;
use a2a_nats::catalog::RegistrarSubject;
use a2a_nats::{A2aAgentId, A2aPrefix};

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
        response
            .bytes()
            .await
            .map(|b| b.to_vec())
            .map_err(|e| BridgeError::UpstreamHttps(e.to_string()))
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
impl<H: JsonHttpPost + Send + Sync, U: OutboundUpstreamUrlResolve> OutboundHttpsAgentUpstream for MappedHttpsUpstream<H, U> {
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
    registrar
        .publish_core(subject.as_str(), card_json)
        .await?;

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
mod tests {
    use super::*;
    use crate::identity::{BridgeUserJwt, CallerHttpsAuth};
    struct MockPoster {
        last_url: std::sync::Mutex<Option<String>>,
    }

    impl MockPoster {
        fn new() -> Self {
            Self {
                last_url: std::sync::Mutex::new(None),
            }
        }

        fn url(&self) -> Option<String> {
            self.last_url.lock().ok().and_then(|g| (*g).clone())
        }
    }

    #[async_trait]
    impl JsonHttpPost for MockPoster {
        async fn post_application_json(&self, url: &str, body: &[u8]) -> Result<Vec<u8>, BridgeError> {
            *self.last_url.lock().unwrap() = Some(url.to_owned());
            assert!(!body.is_empty());
            Ok(br#"{"ok":true}"#.to_vec())
        }
    }

    #[derive(Clone, Default)]
    struct UrlOk;

    #[async_trait]
    impl OutboundUpstreamUrlResolve for UrlOk {
        async fn downstream_post_root(&self, _agent_id: &AgentRegistrationId) -> Result<String, BridgeError> {
            Ok("https://upstream.example/jsonrpc".into())
        }
    }

    #[tokio::test]
    async fn mapped_https_upstream_hits_resolved_root() {
        let poster = Arc::new(MockPoster::new());
        let up: MappedHttpsUpstream<MockPoster, UrlOk> =
            MappedHttpsUpstream::new(poster.clone(), Arc::new(UrlOk));
        let out = OutboundHttpsAgentUpstream::proxy_jsonrpc_post(
            &up,
            AgentRegistrationId::new("ext"),
            MethodSegment::new("message/send"),
            Bytes::from_static(br"{}"),
        )
        .await
        .unwrap();

        assert_eq!(poster.url().as_deref(), Some("https://upstream.example/jsonrpc"));
        assert!(out.starts_with(br#"{"ok""#));
    }

    type RegistrarCapture = std::sync::Arc<std::sync::Mutex<Option<(String, Vec<u8>)>>>;

    #[derive(Clone, Default)]
    struct MockRegistrar(RegistrarCapture);

    #[async_trait]
    impl CatalogRegistrationPublish for MockRegistrar {
        async fn publish_core(&self, subject: impl AsRef<str> + Send, payload: &[u8]) -> Result<(), BridgeError> {
            let mut guard = self.0.lock().map_err(|_| {
                BridgeError::CatalogRegistration("mock registrar mutex poisoned".into())
            })?;
            *guard = Some((subject.as_ref().to_owned(), payload.to_vec()));
            Ok(())
        }
    }

    #[tokio::test]
    async fn catalog_registration_writes_expected_subject() -> Result<(), BridgeError> {
        let captured = Arc::new(std::sync::Mutex::new(None));
        let reg = MockRegistrar(captured.clone());
        let prefix = A2aPrefix::new("a2a".to_string()).unwrap();
        let agent_id = A2aAgentId::new("card-bot").unwrap();
        let card = br#"{"name":"HTTPS proxy agent"}"#;
        publish_https_agent_card_to_catalog(&reg, &prefix, &agent_id, card).await?;
        let g = captured.lock().unwrap();
        let (topic, payload) = g.as_ref().unwrap();
        assert_eq!(topic.as_str(), "a2a.catalog.register.card-bot");
        assert_eq!(payload.as_slice(), card.as_slice());
        Ok(())
    }

    #[test]
    fn identity_newtypes_roundtrip() {
        let j = BridgeUserJwt::new("acct-user-jwt".to_owned());
        assert_eq!(j.as_str(), "acct-user-jwt");
        assert_eq!(j.into_inner(), "acct-user-jwt");

        let c = CallerHttpsAuth::new("Bearer x".to_owned());
        assert_eq!(c.into_inner(), "Bearer x");
    }

    #[test]
    fn agent_and_method_segments_preserve_opaque_strings() {
        let aid = AgentRegistrationId::new("ext-support".to_owned());
        let om = MethodSegment::new("message/send".to_owned());
        assert_eq!(aid.to_string(), "ext-support");
        assert_eq!(om.to_string(), "message/send");
    }
}
