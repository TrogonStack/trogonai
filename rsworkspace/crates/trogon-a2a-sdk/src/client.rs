use std::sync::Arc;

use async_nats::HeaderMap;
use async_trait::async_trait;
use bytes::Bytes;
use opentelemetry::trace::{FutureExt, TraceContextExt, Tracer};
use opentelemetry::{Context, KeyValue, global};
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::constants::{CALLER_JWT_HEADER, DEFAULT_PURPOSE};
use crate::subject::agent_request_subject;
use crate::traits::{MessageTransport, Registry, Sts, SubjectTokenSource, SvidSource};
use crate::types::{AgentId, Audience, ExchangeRequest, Purpose, SdkError};

#[async_trait]
impl MessageTransport for async_nats::Client {
    async fn request(&self, subject: &str, payload: Bytes, headers: HeaderMap) -> Result<Bytes, SdkError> {
        let response = self
            .request_with_headers(subject.to_owned(), headers, payload)
            .await
            .map_err(SdkError::nats)?;
        Ok(response.payload)
    }
}

pub struct Client {
    agent_id: AgentId,
    svid: Arc<dyn SvidSource>,
    subject_token: Arc<dyn SubjectTokenSource>,
    sts: Arc<dyn Sts>,
    registry: Arc<dyn Registry>,
    transport: Arc<dyn MessageTransport>,
}

pub struct ClientBuilder {
    agent_id: Option<AgentId>,
    svid: Option<Arc<dyn SvidSource>>,
    subject_token: Option<Arc<dyn SubjectTokenSource>>,
    sts: Option<Arc<dyn Sts>>,
    registry: Option<Arc<dyn Registry>>,
    transport: Option<Arc<dyn MessageTransport>>,
}

impl ClientBuilder {
    pub fn new() -> Self {
        Self {
            agent_id: None,
            svid: None,
            subject_token: None,
            sts: None,
            registry: None,
            transport: None,
        }
    }

    pub fn agent_id(mut self, agent_id: AgentId) -> Self {
        self.agent_id = Some(agent_id);
        self
    }

    pub fn svid_source(mut self, svid: impl SvidSource + 'static) -> Self {
        self.svid = Some(Arc::new(svid));
        self
    }

    pub fn subject_token_source(mut self, source: impl SubjectTokenSource + 'static) -> Self {
        self.subject_token = Some(Arc::new(source));
        self
    }

    pub fn sts(mut self, sts: impl Sts + 'static) -> Self {
        self.sts = Some(Arc::new(sts));
        self
    }

    pub fn registry(mut self, registry: impl Registry + 'static) -> Self {
        self.registry = Some(Arc::new(registry));
        self
    }

    pub fn transport(mut self, transport: impl MessageTransport + 'static) -> Self {
        self.transport = Some(Arc::new(transport));
        self
    }

    pub fn build(self) -> Result<Client, SdkError> {
        Ok(Client {
            agent_id: self
                .agent_id
                .ok_or_else(|| SdkError::Config("agent_id is required".into()))?,
            svid: self
                .svid
                .ok_or_else(|| SdkError::Config("svid_source is required".into()))?,
            subject_token: self
                .subject_token
                .ok_or_else(|| SdkError::Config("subject_token_source is required".into()))?,
            sts: self.sts.ok_or_else(|| SdkError::Config("sts is required".into()))?,
            registry: self
                .registry
                .ok_or_else(|| SdkError::Config("registry is required".into()))?,
            transport: self
                .transport
                .ok_or_else(|| SdkError::Config("transport is required".into()))?,
        })
    }
}

impl Default for ClientBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl Client {
    pub fn builder() -> ClientBuilder {
        ClientBuilder::new()
    }

    pub async fn call<P, R>(&self, target: &AgentId, payload: &P, purpose: Option<&Purpose>) -> Result<R, SdkError>
    where
        P: Serialize,
        R: DeserializeOwned,
    {
        let tracer = global::tracer("trogon-a2a-sdk");
        let resolved_purpose = self.resolve_purpose(purpose).await?;
        let purpose_str = resolved_purpose.as_str();
        let root = tracer
            .span_builder("a2a.call")
            .with_attributes([
                KeyValue::new("agent.id", self.agent_id.to_string()),
                KeyValue::new("agent.target.id", target.to_string()),
                KeyValue::new("agent.purpose", purpose_str.to_owned()),
                KeyValue::new("agent.call.direction", "outbound"),
            ])
            .start(&tracer);
        let cx = Context::current().with_span(root);
        let parent_cx = cx.clone();

        async {
            let _lookup = tracer
                .span_builder("a2a.call.lookup")
                .start_with_context(&tracer, &parent_cx);
            let record = self.registry.lookup(target).await?;

            let audience = record
                .allowed_audiences
                .first()
                .cloned()
                .unwrap_or_else(|| Audience::for_agent(target).to_string());

            let _exchange = tracer
                .span_builder("a2a.call.exchange")
                .start_with_context(&tracer, &parent_cx);
            let actor_token = self.svid.current().await?;
            let subject_token = self.subject_token.current().await?;
            let scope = if record.allowed_audiences.is_empty() {
                None
            } else {
                Some(record.allowed_audiences.join(" "))
            };
            let exchange = self
                .sts
                .exchange(ExchangeRequest {
                    subject_token,
                    actor_token,
                    audience: audience.clone(),
                    scope,
                    purpose: Some(resolved_purpose.as_str().to_owned()),
                    agent_id: Some(self.agent_id.to_string()),
                })
                .await?;

            let depth = chain_depth_from_token(&exchange.access_token).unwrap_or(0);
            let _chain = tracer
                .span_builder("a2a.call.chain")
                .with_attributes([KeyValue::new("agent.chain.depth", depth as i64)])
                .start_with_context(&tracer, &parent_cx);

            let _send = tracer
                .span_builder("a2a.call.send")
                .start_with_context(&tracer, &parent_cx);
            let subject = agent_request_subject(target);
            let body = serde_json::to_vec(payload).map_err(|e| SdkError::Serialization(e.to_string()))?;
            let mut headers = HeaderMap::new();
            headers.insert(CALLER_JWT_HEADER, exchange.access_token.as_str());
            let reply = self.transport.request(&subject, body.into(), headers).await?;

            serde_json::from_slice(&reply).map_err(|e| SdkError::Serialization(e.to_string()))
        }
        .with_context(cx)
        .await
    }

    async fn resolve_purpose(&self, purpose: Option<&Purpose>) -> Result<Purpose, SdkError> {
        if let Some(purpose) = purpose {
            return Ok(purpose.clone());
        }
        let record = self.registry.lookup(&self.agent_id).await?;
        match record.allowed_purposes.len() {
            0 => Ok(Purpose::new(DEFAULT_PURPOSE)),
            1 => Ok(Purpose::new(record.allowed_purposes[0].clone())),
            _ => Err(SdkError::PurposeRequired),
        }
    }
}

fn chain_depth_from_token(token: &str) -> Option<usize> {
    let payload = token.split('.').nth(1)?;
    use base64::Engine;
    let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(payload).ok()?;
    let claims: serde_json::Value = serde_json::from_slice(&decoded).ok()?;
    claims
        .get("act_chain")
        .and_then(|c| c.as_array())
        .map(std::vec::Vec::len)
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use async_nats::HeaderMap;
    use async_trait::async_trait;
    use bytes::Bytes;
    use serde::{Deserialize, Serialize};

    use super::*;
    use crate::constants::{CALLER_JWT_HEADER, DEFAULT_PURPOSE};
    use crate::subject::agent_request_subject;
    use crate::types::{AgentRecord, ExchangeRequest, ExchangeResponse};

    struct FixedSvid(String);
    #[async_trait]
    impl SvidSource for FixedSvid {
        async fn current(&self) -> Result<String, SdkError> {
            Ok(self.0.clone())
        }
    }

    struct FixedSubjectToken(String);
    #[async_trait]
    impl SubjectTokenSource for FixedSubjectToken {
        async fn current(&self) -> Result<String, SdkError> {
            Ok(self.0.clone())
        }
    }

    struct MockSts {
        last_req: Arc<Mutex<Option<ExchangeRequest>>>,
        token: String,
    }

    #[async_trait]
    impl Sts for MockSts {
        async fn exchange(&self, req: ExchangeRequest) -> Result<ExchangeResponse, SdkError> {
            *self.last_req.lock().unwrap() = Some(req);
            Ok(ExchangeResponse {
                access_token: self.token.clone(),
                expires_in: Some(120),
                token_type: Some("Bearer".into()),
            })
        }
    }

    struct MockRegistry {
        self_record: AgentRecord,
        target_record: AgentRecord,
    }

    #[async_trait]
    impl Registry for MockRegistry {
        async fn lookup(&self, agent_id: &AgentId) -> Result<AgentRecord, SdkError> {
            if agent_id.as_str() == "acme/oncall" {
                Ok(self.self_record.clone())
            } else {
                Ok(self.target_record.clone())
            }
        }
    }

    #[derive(Default)]
    struct CapturedRequest {
        subject: String,
        headers: HeaderMap,
        payload: Bytes,
    }

    struct MockTransport {
        captured: Arc<Mutex<Option<CapturedRequest>>>,
        response: Bytes,
    }

    #[async_trait]
    impl MessageTransport for MockTransport {
        async fn request(&self, subject: &str, payload: Bytes, headers: HeaderMap) -> Result<Bytes, SdkError> {
            *self.captured.lock().unwrap() = Some(CapturedRequest {
                subject: subject.to_owned(),
                headers,
                payload,
            });
            Ok(self.response.clone())
        }
    }

    #[derive(Serialize, Deserialize, PartialEq, Eq, Debug)]
    struct EchoReq {
        msg: String,
    }

    #[derive(Serialize, Deserialize, PartialEq, Eq, Debug)]
    struct EchoResp {
        echo: String,
    }

    #[tokio::test]
    async fn call_publishes_to_expected_subject_with_caller_jwt_header() {
        let _guard = crate::test_tracer_lock::tracer_test_guard().await;
        let target = AgentId::parse("acme/planner").unwrap();
        let mesh_token = "header.payload.sig";
        let (sts, last_req) = mock_sts(mesh_token);
        let captured = Arc::new(Mutex::new(None));
        let transport = MockTransport {
            captured: captured.clone(),
            response: br#"{"echo":"hi"}"#.as_slice().into(),
        };
        let client = Client::builder()
            .agent_id(AgentId::parse("acme/oncall").unwrap())
            .svid_source(FixedSvid("svid-jwt".into()))
            .subject_token_source(FixedSubjectToken("bootstrap-jwt".into()))
            .sts(sts)
            .registry(MockRegistry {
                self_record: AgentRecord {
                    allowed_audiences: vec![],
                    allowed_purposes: vec![],
                    mesh_token_ttl_s: None,
                },
                target_record: AgentRecord {
                    allowed_audiences: vec!["urn:trogon:a2a:agent:acme:planner".into()],
                    allowed_purposes: vec![],
                    mesh_token_ttl_s: Some(120),
                },
            })
            .transport(transport)
            .build()
            .unwrap();

        let resp: EchoResp = client
            .call(&target, &EchoReq { msg: "hi".into() }, Some(&Purpose::new("handoff")))
            .await
            .unwrap();
        assert_eq!(resp, EchoResp { echo: "hi".into() });

        let captured = captured.lock().unwrap().take().unwrap();
        assert_eq!(captured.subject, agent_request_subject(&target));
        assert_eq!(captured.headers.get(CALLER_JWT_HEADER).unwrap().as_str(), mesh_token);
        let req_body: EchoReq = serde_json::from_slice(&captured.payload).unwrap();
        assert_eq!(req_body.msg, "hi");
        let _ = last_req;
    }

    fn test_client(sts: MockSts, registry: MockRegistry, transport: MockTransport) -> Client {
        Client::builder()
            .agent_id(AgentId::parse("acme/oncall").unwrap())
            .svid_source(FixedSvid("svid-jwt".into()))
            .subject_token_source(FixedSubjectToken("bootstrap-jwt".into()))
            .sts(sts)
            .registry(registry)
            .transport(transport)
            .build()
            .unwrap()
    }

    fn mock_sts(token: &str) -> (MockSts, Arc<Mutex<Option<ExchangeRequest>>>) {
        let last_req = Arc::new(Mutex::new(None));
        (
            MockSts {
                last_req: Arc::clone(&last_req),
                token: token.to_owned(),
            },
            last_req,
        )
    }

    #[tokio::test]
    async fn call_uses_default_purpose_when_registry_has_none() {
        let _guard = crate::test_tracer_lock::tracer_test_guard().await;
        let (sts, last_req) = mock_sts("mesh");
        let client = test_client(
            sts,
            MockRegistry {
                self_record: AgentRecord {
                    allowed_audiences: vec![],
                    allowed_purposes: vec![],
                    mesh_token_ttl_s: None,
                },
                target_record: AgentRecord {
                    allowed_audiences: vec!["urn:trogon:a2a:agent:acme:planner".into()],
                    allowed_purposes: vec![],
                    mesh_token_ttl_s: None,
                },
            },
            MockTransport {
                captured: Arc::new(Mutex::new(None)),
                response: br#"{"echo":"ok"}"#.as_slice().into(),
            },
        );
        let target = AgentId::parse("acme/planner").unwrap();
        let _: EchoResp = client.call(&target, &EchoReq { msg: "x".into() }, None).await.unwrap();
        let req = last_req.lock().unwrap().take().unwrap();
        assert_eq!(req.purpose.as_deref(), Some(DEFAULT_PURPOSE));
    }

    #[tokio::test]
    async fn call_uses_sole_allowed_purpose_when_omitted() {
        let _guard = crate::test_tracer_lock::tracer_test_guard().await;
        let (sts, last_req) = mock_sts("mesh");
        let client = test_client(
            sts,
            MockRegistry {
                self_record: AgentRecord {
                    allowed_audiences: vec![],
                    allowed_purposes: vec!["triage".into()],
                    mesh_token_ttl_s: None,
                },
                target_record: AgentRecord {
                    allowed_audiences: vec!["urn:trogon:a2a:agent:acme:planner".into()],
                    allowed_purposes: vec![],
                    mesh_token_ttl_s: None,
                },
            },
            MockTransport {
                captured: Arc::new(Mutex::new(None)),
                response: br#"{"echo":"ok"}"#.as_slice().into(),
            },
        );
        let target = AgentId::parse("acme/planner").unwrap();
        let _: EchoResp = client.call(&target, &EchoReq { msg: "x".into() }, None).await.unwrap();
        let req = last_req.lock().unwrap().take().unwrap();
        assert_eq!(req.purpose.as_deref(), Some("triage"));
    }

    #[tokio::test]
    async fn call_errors_when_multiple_allowed_purposes_and_none_passed() {
        let _guard = crate::test_tracer_lock::tracer_test_guard().await;
        let (sts, _) = mock_sts("mesh");
        let client = test_client(
            sts,
            MockRegistry {
                self_record: AgentRecord {
                    allowed_audiences: vec![],
                    allowed_purposes: vec!["a".into(), "b".into()],
                    mesh_token_ttl_s: None,
                },
                target_record: AgentRecord {
                    allowed_audiences: vec!["urn:trogon:a2a:agent:acme:planner".into()],
                    allowed_purposes: vec![],
                    mesh_token_ttl_s: None,
                },
            },
            MockTransport {
                captured: Arc::new(Mutex::new(None)),
                response: br#"{"echo":"ok"}"#.as_slice().into(),
            },
        );
        let target = AgentId::parse("acme/planner").unwrap();
        let err = client
            .call::<EchoReq, EchoResp>(&target, &EchoReq { msg: "x".into() }, None)
            .await
            .unwrap_err();
        assert!(matches!(err, SdkError::PurposeRequired));
    }
}
