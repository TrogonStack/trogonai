use super::*;
use crate::identity::{BridgeAgentId, BridgeUserJwt, CallerHttpsAuth};
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
    let up: MappedHttpsUpstream<MockPoster, UrlOk> = MappedHttpsUpstream::new(poster.clone(), Arc::new(UrlOk));
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
        let mut guard = self
            .0
            .lock()
            .map_err(|_| BridgeError::CatalogRegistration("mock registrar mutex poisoned".into()))?;
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
    let j = BridgeUserJwt::new("hhh.ppp.sss".to_owned()).expect("valid jwt shape");
    assert_eq!(j.as_str(), "hhh.ppp.sss");
    assert_eq!(j.into_inner(), "hhh.ppp.sss");

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

#[tokio::test]
async fn stub_outbound_forwarder_returns_upstream_error() {
    let err = StubOutboundForwarder
        .proxy_jsonrpc_post(
            AgentRegistrationId::new("agent"),
            MethodSegment::new("message/send"),
            Bytes::from_static(b"{}"),
        )
        .await
        .unwrap_err();
    assert!(matches!(err, BridgeError::UpstreamHttps(_)));
}

#[tokio::test]
async fn forward_delegates_to_upstream() {
    let poster = Arc::new(MockPoster::new());
    let up: MappedHttpsUpstream<MockPoster, UrlOk> = MappedHttpsUpstream::new(poster.clone(), Arc::new(UrlOk));
    let out = forward(
        &up,
        AgentRegistrationId::new("ext"),
        MethodSegment::new("tasks/get"),
        Bytes::from_static(br#"{"id":1}"#),
    )
    .await
    .unwrap();
    assert!(out.starts_with(br#"{"ok""#));
}

#[tokio::test]
async fn publish_https_agent_card_registered_uses_bridge_agent_id() -> Result<(), BridgeError> {
    let captured = Arc::new(std::sync::Mutex::new(None));
    let reg = MockRegistrar(captured.clone());
    let prefix = A2aPrefix::new("a2a".to_string()).unwrap();
    let agent_id = BridgeAgentId::parse("card-bot").unwrap();
    let card = br#"{"name":"bridge agent"}"#;
    publish_https_agent_card_registered(&reg, &prefix, &agent_id, card).await?;
    let g = captured.lock().unwrap();
    let (topic, payload) = g.as_ref().unwrap();
    assert_eq!(topic.as_str(), "a2a.catalog.register.card-bot");
    assert_eq!(payload.as_slice(), card.as_slice());
    Ok(())
}

#[test]
fn reqwest_default_https_client_builds() {
    assert!(ReqwestJsonHttpPoster::default_https_client().is_ok());
}

#[derive(Clone, Default)]
struct UrlErr;

#[async_trait]
impl OutboundUpstreamUrlResolve for UrlErr {
    async fn downstream_post_root(&self, _: &AgentRegistrationId) -> Result<String, BridgeError> {
        Err(BridgeError::UpstreamHttps("resolve failed".into()))
    }
}

#[derive(Clone, Default)]
struct PostErr;

#[async_trait]
impl JsonHttpPost for PostErr {
    async fn post_application_json(&self, _: &str, _: &[u8]) -> Result<Vec<u8>, BridgeError> {
        Err(BridgeError::UpstreamHttps("post failed".into()))
    }
}

#[derive(Clone, Default)]
struct FailingRegistrar;

#[async_trait]
impl CatalogRegistrationPublish for FailingRegistrar {
    async fn publish_core(&self, _: impl AsRef<str> + Send, _: &[u8]) -> Result<(), BridgeError> {
        Err(BridgeError::CatalogRegistration("publish failed".into()))
    }
}

#[tokio::test]
async fn mapped_https_upstream_surfaces_resolve_error() {
    let up: MappedHttpsUpstream<MockPoster, UrlErr> =
        MappedHttpsUpstream::new(Arc::new(MockPoster::new()), Arc::new(UrlErr));
    let err = up
        .proxy_jsonrpc_post(
            AgentRegistrationId::new("ext"),
            MethodSegment::new("message/send"),
            Bytes::from_static(b"{}"),
        )
        .await
        .unwrap_err();
    assert!(matches!(err, BridgeError::UpstreamHttps(_)));
}

#[tokio::test]
async fn mapped_https_upstream_surfaces_post_error() {
    let up: MappedHttpsUpstream<PostErr, UrlOk> = MappedHttpsUpstream::new(Arc::new(PostErr), Arc::new(UrlOk));
    let err = up
        .proxy_jsonrpc_post(
            AgentRegistrationId::new("ext"),
            MethodSegment::new("message/send"),
            Bytes::from_static(b"{}"),
        )
        .await
        .unwrap_err();
    assert!(matches!(err, BridgeError::UpstreamHttps(_)));
}

#[tokio::test]
async fn publish_https_agent_card_to_catalog_surfaces_registrar_error() {
    let prefix = A2aPrefix::new("a2a".to_string()).unwrap();
    let agent_id = A2aAgentId::new("card-bot").unwrap();
    let err = publish_https_agent_card_to_catalog(&FailingRegistrar, &prefix, &agent_id, b"{}")
        .await
        .unwrap_err();
    assert!(matches!(err, BridgeError::CatalogRegistration(_)));
}

#[tokio::test]
async fn reqwest_poster_returns_success_body() {
    let server = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path("/jsonrpc"))
        .respond_with(wiremock::ResponseTemplate::new(200).set_body_raw(r#"{"ok":true}"#, "application/json"))
        .mount(&server)
        .await;

    let poster = ReqwestJsonHttpPoster::new(reqwest::Client::builder().build().unwrap());
    let url = format!("{}/jsonrpc", server.uri());
    let body = poster.post_application_json(&url, br#"{}"#).await.unwrap();
    assert_eq!(body, br#"{"ok":true}"#);
}

#[tokio::test]
async fn reqwest_poster_surfaces_transport_error() {
    let poster = ReqwestJsonHttpPoster::new(reqwest::Client::builder().build().unwrap());
    let err = poster
        .post_application_json("http://127.0.0.1:1/jsonrpc", b"{}")
        .await
        .unwrap_err();
    assert!(matches!(err, BridgeError::UpstreamHttps(_)));
}

#[tokio::test]
async fn reqwest_poster_rejects_non_success_status() {
    let server = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .respond_with(wiremock::ResponseTemplate::new(502).set_body_raw("bad gateway", "text/plain"))
        .mount(&server)
        .await;

    let poster = ReqwestJsonHttpPoster::new(reqwest::Client::builder().build().unwrap());
    let err = poster.post_application_json(&server.uri(), b"{}").await.unwrap_err();
    assert!(matches!(err, BridgeError::UpstreamHttps(_)));
}

#[test]
fn agent_and_method_segments_expose_as_str() {
    let aid = AgentRegistrationId::new("ext-support");
    let method = MethodSegment::new("message/send");
    assert_eq!(aid.as_str(), "ext-support");
    assert_eq!(method.as_str(), "message/send");
}
