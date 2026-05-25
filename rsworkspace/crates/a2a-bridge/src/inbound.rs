use std::collections::VecDeque;
use std::convert::Infallible;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::Duration;

use async_trait::async_trait;
use axum::{
    Router,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{
        IntoResponse, Response,
        sse::{Event, KeepAlive, Sse},
    },
    routing::post,
};
use bytes::Bytes;
use futures_util::StreamExt;
use futures_util::stream::{self, BoxStream, Stream};
use serde_json::{Value, json};

use a2a_nats::constants::REQ_ID_HEADER;
use a2a_nats::jetstream::consumers::{resubscribe_consumer, stream_events_consumer};
use a2a_nats::jetstream::streams::events_stream_name;
use a2a_nats::{A2aPrefix, A2aTaskId, ReqId};

use a2a_auth_callout::{CallerJwtHeaderValue, MintedUserJwt, CALLER_JWT_HEADER_NAME};

use crate::auth::AuthCalloutClient;
use crate::error::BridgeError;
use crate::identity::{BridgeAgentId, BridgeUserJwt, CallerHttpsAuth};

const AGENT_ID_HEADER: &str = "x-a2a-agent-id";

pub fn gateway_method_to_subject_dots(method: &str) -> String {
    method.replace('/', ".")
}

#[must_use]
pub fn default_a2a_prefix() -> A2aPrefix {
    A2aPrefix::new(String::from("a2a")).expect("embedded bridge default prefix validates")
}

#[must_use]
pub fn build_gateway_subject(prefix: &A2aPrefix, agent_id: &str, method: &str) -> String {
    format!(
        "{}.gateway.{}.{}",
        prefix.as_str(),
        agent_id,
        gateway_method_to_subject_dots(method)
    )
}

#[must_use]
pub fn task_events_wild_subject(prefix: &A2aPrefix, task_id: &str) -> String {
    format!("{}.task.{task_id}.events.>", prefix.as_str())
}

pub fn is_sse_jsonrpc_method(method: &str) -> bool {
    matches!(method, "message/stream" | "tasks/resubscribe")
}

#[derive(Clone)]
pub struct AppState {
    pub(crate) auth: Arc<dyn AuthCalloutClient>,
    pub(crate) publisher: Arc<dyn InboundGatewayPublish>,
    pub(crate) jetstream: Arc<dyn TaskJetStreamPort>,
    pub(crate) prefix: A2aPrefix,
}

impl AppState {
    #[must_use]
    pub fn new(
        auth: Arc<dyn AuthCalloutClient>,
        publisher: Arc<dyn InboundGatewayPublish>,
        jetstream: Arc<dyn TaskJetStreamPort>,
        prefix: A2aPrefix,
    ) -> Self {
        Self {
            auth,
            publisher,
            jetstream,
            prefix,
        }
    }
}

#[async_trait]
pub trait InboundGatewayPublish: Send + Sync {
    async fn publish_unary_to_gateway(
        &self,
        subject: &str,
        caller_jwt: &BridgeUserJwt,
        nats_headers: async_nats::HeaderMap,
        jsonrpc_payload: &[u8],
    ) -> Result<Bytes, BridgeError>;
}

#[derive(Clone, Copy, Debug)]
pub struct StubInboundGatewayPublish;

#[async_trait]
impl InboundGatewayPublish for StubInboundGatewayPublish {
    async fn publish_unary_to_gateway(
        &self,
        _subject: &str,
        _caller_jwt: &BridgeUserJwt,
        _nats_headers: async_nats::HeaderMap,
        _jsonrpc_payload: &[u8],
    ) -> Result<Bytes, BridgeError> {
        Err(BridgeError::NatsPublish(
            "gateway publish backed by StubInboundGatewayPublish".into(),
        ))
    }
}

#[derive(Clone)]
pub struct RecordingInboundPublisher {
    pub last_subject: Arc<Mutex<Option<String>>>,
}

impl RecordingInboundPublisher {
    #[must_use]
    pub fn new() -> Self {
        Self {
            last_subject: Arc::new(Mutex::new(None)),
        }
    }

    #[must_use]
    pub fn peek_subject(&self) -> Option<String> {
        self.last_subject.lock().ok().and_then(|g| (*g).clone())
    }
}

impl Default for RecordingInboundPublisher {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl InboundGatewayPublish for RecordingInboundPublisher {
    async fn publish_unary_to_gateway(
        &self,
        subject: &str,
        _caller_jwt: &BridgeUserJwt,
        _nats_headers: async_nats::HeaderMap,
        _jsonrpc_payload: &[u8],
    ) -> Result<Bytes, BridgeError> {
        if let Ok(mut g) = self.last_subject.lock() {
            *g = Some(subject.to_owned());
        }
        Ok(Bytes::from(static_json_ok()))
    }
}

fn static_json_ok() -> Vec<u8> {
    serde_json::to_vec(&json!({
        "jsonrpc": "2.0",
        "id": null,
        "result": {},
    }))
    .unwrap_or_else(|_| b"{}".to_vec())
}

#[async_trait]
pub trait GatewayUnaryPublish: Send + Sync {
    async fn unary_request_gateway(
        &self,
        caller_jwt: &BridgeUserJwt,
        subject: &str,
        headers: async_nats::HeaderMap,
        payload: Bytes,
    ) -> Result<Bytes, BridgeError>;
}

#[derive(Clone)]
pub struct AsyncNatsTokenGatewayUnary {
    servers: Arc<[String]>,
    timeout: Duration,
}

impl AsyncNatsTokenGatewayUnary {
    #[must_use]
    pub fn new(servers: Vec<String>, timeout: Duration) -> Self {
        Self {
            servers: servers.into_boxed_slice().into(),
            timeout,
        }
    }

    #[must_use]
    pub fn from_single_url(server: impl Into<String>, timeout: Duration) -> Self {
        Self::new(vec![server.into()], timeout)
    }
}

#[async_trait]
impl GatewayUnaryPublish for AsyncNatsTokenGatewayUnary {
    async fn unary_request_gateway(
        &self,
        caller_jwt: &BridgeUserJwt,
        subject: &str,
        headers: async_nats::HeaderMap,
        payload: Bytes,
    ) -> Result<Bytes, BridgeError> {
        let client = async_nats::ConnectOptions::new()
            .connection_timeout(self.timeout)
            .token(caller_jwt.as_str().to_owned())
            .connect(&self.servers[..])
            .await
            .map_err(|e| BridgeError::NatsPublish(e.to_string()))?;
        client
            .request_with_headers(subject.to_owned(), headers, payload)
            .await
            .map_err(|e| BridgeError::NatsPublish(e.to_string()))
            .map(|reply| reply.payload)
    }
}

#[derive(Clone)]
pub struct GatewayInboundPublisher<G> {
    inner: Arc<G>,
}

impl<G> GatewayInboundPublisher<G> {
    #[must_use]
    pub fn new(inner: Arc<G>) -> Self {
        Self { inner }
    }
}

#[async_trait]
impl<G: GatewayUnaryPublish + 'static> InboundGatewayPublish for GatewayInboundPublisher<G> {
    async fn publish_unary_to_gateway(
        &self,
        subject: &str,
        caller_jwt: &BridgeUserJwt,
        nats_headers: async_nats::HeaderMap,
        jsonrpc_payload: &[u8],
    ) -> Result<Bytes, BridgeError> {
        self.inner
            .unary_request_gateway(
                caller_jwt,
                subject,
                nats_headers,
                Bytes::copy_from_slice(jsonrpc_payload),
            )
            .await
    }
}

#[derive(Clone, Debug)]
pub enum SseConsumePlan {
    MessageStreamBootstrap { req_id: ReqId },
    TasksResubscribe { task_id: A2aTaskId, last_seq: u64 },
}

#[async_trait]
pub trait TaskJetStreamPort: Send + Sync {
    async fn task_event_payload_stream(
        &self,
        caller_jwt: &BridgeUserJwt,
        prefix: &A2aPrefix,
        plan: SseConsumePlan,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<Bytes, BridgeError>> + Send>>, BridgeError>;
}

#[derive(Clone, Copy, Debug)]
pub struct StubTaskJetStreamPort;

#[async_trait]
impl TaskJetStreamPort for StubTaskJetStreamPort {
    async fn task_event_payload_stream(
        &self,
        _caller_jwt: &BridgeUserJwt,
        _prefix: &A2aPrefix,
        _plan: SseConsumePlan,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<Bytes, BridgeError>> + Send>>, BridgeError> {
        Err(BridgeError::JetStreamConsume(
            "task jetstream backed by StubTaskJetStreamPort".into(),
        ))
    }
}

#[derive(Clone)]
pub struct AsyncNatsTokenTaskJetstream {
    servers: Arc<[String]>,
    timeout: Duration,
}

impl AsyncNatsTokenTaskJetstream {
    #[must_use]
    pub fn new(servers: Vec<String>, timeout: Duration) -> Self {
        Self {
            servers: servers.into_boxed_slice().into(),
            timeout,
        }
    }

    #[must_use]
    pub fn from_single_url(server: impl Into<String>, timeout: Duration) -> Self {
        Self::new(vec![server.into()], timeout)
    }

    async fn connect_caller_client(&self, jwt: &BridgeUserJwt) -> Result<async_nats::Client, BridgeError> {
        async_nats::ConnectOptions::new()
            .connection_timeout(self.timeout)
            .token(jwt.as_str().to_owned())
            .connect(&self.servers[..])
            .await
            .map_err(|e| BridgeError::JetStreamConsume(e.to_string()))
    }
}

#[async_trait]
impl TaskJetStreamPort for AsyncNatsTokenTaskJetstream {
    async fn task_event_payload_stream(
        &self,
        caller_jwt: &BridgeUserJwt,
        prefix: &A2aPrefix,
        plan: SseConsumePlan,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<Bytes, BridgeError>> + Send>>, BridgeError> {
        let client = self.connect_caller_client(caller_jwt).await?;
        let jetstream_ctx = async_nats::jetstream::new(client);
        let stream_name = events_stream_name(prefix);
        let stream = jetstream_ctx
            .get_stream(stream_name.clone())
            .await
            .map_err(|e| BridgeError::JetStreamConsume(format!("get_stream {stream_name}: {e}")))?;
        let pull_cfg = match plan {
            SseConsumePlan::MessageStreamBootstrap { ref req_id } => {
                stream_events_consumer(prefix, req_id)
            }
            SseConsumePlan::TasksResubscribe {
                ref task_id,
                last_seq,
            } => resubscribe_consumer(prefix, task_id, last_seq),
        };
        let consumer = stream
            .create_consumer(pull_cfg)
            .await
            .map_err(|e| BridgeError::JetStreamConsume(format!("create_consumer: {e}")))?;

        let mut messages_stream = consumer
            .messages()
            .await
            .map_err(|e| BridgeError::JetStreamConsume(format!("consumer.messages: {e}")))?;

        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();

        tokio::spawn(async move {
            while let Some(item) = messages_stream.next().await {
                match item {
                    Ok(js_message) => {
                        let chunk = Bytes::clone(&js_message.message.payload);
                        if let Err(ack_err) = js_message.ack().await {
                            let _ = tx.send(Err(BridgeError::JetStreamConsume(ack_err.to_string())));
                            break;
                        }
                        if tx.send(Ok(chunk)).is_err() {
                            break;
                        }
                    }
                    Err(err) => {
                        let _ = tx.send(Err(BridgeError::JetStreamConsume(err.to_string())));
                        break;
                    }
                }
            }
        });

        Ok(Box::pin(RxPollStream(rx)))
    }
}

struct RxPollStream(
    tokio::sync::mpsc::UnboundedReceiver<Result<Bytes, BridgeError>>,
);

impl Stream for RxPollStream {
    type Item = Result<Bytes, BridgeError>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.0.poll_recv(cx)
    }
}

#[derive(Clone)]
pub struct ScriptedTaskJetstream {
    queue: Arc<Mutex<VecDeque<Result<Bytes, BridgeError>>>>,
}

impl ScriptedTaskJetstream {
    #[must_use]
    pub fn from_script(items: Vec<Result<Bytes, BridgeError>>) -> Self {
        Self {
            queue: Arc::new(Mutex::new(VecDeque::from(items))),
        }
    }

    #[must_use]
    pub fn single_ok(payload: impl Into<Vec<u8>>) -> Self {
        Self::from_script(vec![Ok(Bytes::from(payload.into()))])
    }
}

#[async_trait]
impl TaskJetStreamPort for ScriptedTaskJetstream {
    async fn task_event_payload_stream(
        &self,
        _caller_jwt: &BridgeUserJwt,
        _prefix: &A2aPrefix,
        _plan: SseConsumePlan,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<Bytes, BridgeError>> + Send>>, BridgeError> {
        let drained: VecDeque<_> = {
            let Ok(mut guard) = self.queue.lock() else {
                return Err(BridgeError::JetStreamConsume(
                    "scripted mutex poisoned".into(),
                ));
            };
            std::mem::take(&mut (*guard))
        };
        Ok(Box::pin(stream::iter(drained)))
    }
}

fn sse_gateway_line(body: &[u8]) -> Event {
    Event::default()
        .event("gateway-bootstrap")
        .data(String::from_utf8_lossy(body))
}

fn sse_task_line(body: &Bytes) -> Event {
    Event::default()
        .event("task-event")
        .data(String::from_utf8_lossy(body.as_ref()))
}

fn sse_error_line(err: &BridgeError) -> Event {
    Event::default().event("error").data(err.to_string())
}

fn sse_from_bootstrap_and_payloads(
    bootstrap_owned: Vec<u8>,
    tail: Pin<Box<dyn Stream<Item = Result<Bytes, BridgeError>> + Send>>,
) -> BoxStream<'static, Result<Event, Infallible>> {
    let head_event = sse_gateway_line(&bootstrap_owned);
    let head = futures_util::stream::once(futures_util::future::ready(Ok::<Event, Infallible>(head_event)));
    let tail_mapped = tail.map(|item| {
        Ok::<Event, Infallible>(match item {
            Ok(chunk) => sse_task_line(&chunk),
            Err(ref err) => sse_error_line(err),
        })
    });

    Box::pin(head.chain(tail_mapped))
}

fn gateway_req_headers(correlation: ReqId) -> Result<async_nats::HeaderMap, BridgeError> {
    let mut map = async_nats::HeaderMap::new();
    let value = correlation.as_str();
    map.insert(
        REQ_ID_HEADER,
        async_nats::header::HeaderValue::from(value),
    );

    Ok(map)
}

/// Gateway publish headers propagate JSON-RPC correlation and the auth-callout minted User JWT.
pub fn gateway_publish_headers(
    correlation: ReqId,
    caller_jwt: &BridgeUserJwt,
) -> Result<async_nats::HeaderMap, BridgeError> {
    let mut map = gateway_req_headers(correlation)?;
    let minted = MintedUserJwt::new(caller_jwt.as_str());
    let header_value = CallerJwtHeaderValue::from_minted(&minted);
    let nats_name = async_nats::header::HeaderName::from_static(CALLER_JWT_HEADER_NAME);
    map.insert(
        nats_name,
        async_nats::header::HeaderValue::from(header_value.as_str()),
    );
    Ok(map)
}

fn caller_auth_from(headers: &HeaderMap) -> Result<CallerHttpsAuth, BridgeError> {
    let raw = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .ok_or(BridgeError::MissingAuthorization)?;
    Ok(CallerHttpsAuth::new(raw.to_owned()))
}

fn agent_header_parse(headers: &HeaderMap) -> Result<BridgeAgentId, BridgeError> {
    let raw = headers
        .get(AGENT_ID_HEADER)
        .and_then(|v| v.to_str().ok())
        .ok_or(BridgeError::MissingAgentHeader)?;
    BridgeAgentId::parse(raw)
}

fn json_rpc_corr_id(payload: &Value) -> ReqId {
    payload
        .get("id")
        .map(|v| match v {
            Value::String(s) => ReqId::from_header(s.clone()),
            Value::Number(n) => ReqId::from_header(n.to_string()),
            Value::Bool(b) => ReqId::from_header(b.to_string()),
            Value::Null | Value::Array(_) | Value::Object(_) => ReqId::new(),
        })
        .unwrap_or_default()
}

fn extract_last_sequence(params: &Value) -> Option<u64> {
    for key in [
        "last_known_sequence_number",
        "lastSequence",
        "last_sequence",
        "last_seq",
        "resume_from_sequence",
        "resumeFromSeq",
    ] {
        if let Some(v) = params.get(key) {
            if let Some(n) = v.as_u64() {
                return Some(n);
            }
            if let Some(s) = v.as_str()
                && let Ok(parsed) = s.parse::<u64>()
            {
                return Some(parsed);
            }
        }
    }
    None
}

fn resub_task_and_seq(body: &Value) -> Result<(A2aTaskId, u64), BridgeError> {
    let params = body.get("params").ok_or_else(|| {
        BridgeError::StreamingParams("tasks/resubscribe expects params envelope".into())
    })?;
    let tid = params
        .get("id")
        .and_then(Value::as_str)
        .or_else(|| params.get("task_id").and_then(Value::as_str))
        .or_else(|| params.get("taskId").and_then(Value::as_str))
        .ok_or_else(|| BridgeError::StreamingParams("tasks/resubscribe missing task identifier".into()))?;
    let task_id = A2aTaskId::new(tid).map_err(|e| BridgeError::StreamingParams(e.to_string()))?;
    Ok((task_id, extract_last_sequence(params).unwrap_or(0)))
}

fn sse_plan(method: &str, body: &Value) -> Result<SseConsumePlan, BridgeError> {
    match method {
        "message/stream" => Ok(SseConsumePlan::MessageStreamBootstrap {
            req_id: json_rpc_corr_id(body),
        }),
        "tasks/resubscribe" => {
            let (task_id, last_seq) = resub_task_and_seq(body)?;
            Ok(SseConsumePlan::TasksResubscribe { task_id, last_seq })
        }
        other => Err(BridgeError::StreamingParams(format!(
            "unsupported streaming method {other}"
        ))),
    }
}

fn assert_jsonrpc_gateway_ok(slice: &[u8]) -> Result<(), BridgeError> {
    let v: Value =
        serde_json::from_slice(slice).map_err(|e: serde_json::Error| BridgeError::Deserialize(e))?;
    if v.get("error").is_some() {
        let detail = serde_json::to_string(&v["error"]).unwrap_or_else(|_| "{}".into());
        return Err(BridgeError::JsonRpcUpstream(detail));
    }
    Ok(())
}

pub fn gateway_router(state: AppState) -> Router {
    Router::new().route("/", post(a2a_post)).with_state(state)
}

async fn a2a_post(State(state): State<AppState>, headers: HeaderMap, body: bytes::Bytes) -> Response {
    match handle_jsonrpc(headers, body, &state).await {
        Ok(r) => r,
        Err(e) => bridge_error_into_response(e),
    }
}

fn bridge_error_into_response(e: BridgeError) -> Response {
    let status = StatusCode::BAD_GATEWAY;
    let payload = serde_json::json!({"error": e.to_string()});
    (status, axum::Json(payload)).into_response()
}

pub async fn handle_jsonrpc(headers: HeaderMap, body: bytes::Bytes, state: &AppState) -> Result<Response, BridgeError> {
    let caller_auth = caller_auth_from(&headers)?;
    let agent_id = agent_header_parse(&headers)?;
    let jwt = state.auth.mint(&caller_auth).await?;
    let v: Value =
        serde_json::from_slice(&body).map_err(|e: serde_json::Error| BridgeError::Deserialize(e))?;
    let Some(method) = v.get("method").and_then(Value::as_str) else {
        return Err(BridgeError::MissingJsonRpcMethod);
    };
    let subject = build_gateway_subject(&state.prefix, agent_id.as_str(), method);

    if is_sse_jsonrpc_method(method) {
        let nats_headers = gateway_publish_headers(json_rpc_corr_id(&v), &jwt)?;
        let unary_reply = state
            .publisher
            .publish_unary_to_gateway(&subject, &jwt, nats_headers, body.as_ref())
            .await?;
        assert_jsonrpc_gateway_ok(&unary_reply)?;
        let plan = sse_plan(method, &v)?;
        let payloads = state
            .jetstream
            .task_event_payload_stream(&jwt, &state.prefix, plan)
            .await?;
        let merged = sse_from_bootstrap_and_payloads(unary_reply.to_vec(), payloads);
        return Ok(Sse::new(merged).keep_alive(KeepAlive::default()).into_response());
    }

    let reply = state
        .publisher
        .publish_unary_to_gateway(
            &subject,
            &jwt,
            gateway_publish_headers(json_rpc_corr_id(&v), &jwt)?,
            body.as_ref(),
        )
        .await?;
    Response::builder()
        .status(StatusCode::OK)
        .header(axum::http::header::CONTENT_TYPE, "application/json")
        .body(axum::body::Body::from(reply))
        .map_err(|e| BridgeError::NatsPublish(e.to_string()))
}
