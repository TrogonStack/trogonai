use std::collections::VecDeque;
use std::convert::Infallible;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::Duration;

use async_trait::async_trait;
use axum::{
    Router,
    extract::{OriginalUri, State},
    http::{HeaderMap, Method, StatusCode, header::WWW_AUTHENTICATE},
    response::{
        IntoResponse, Response,
        sse::{Event, KeepAlive, Sse},
    },
    routing::post,
};
use bytes::Bytes;
use futures_util::StreamExt;
use futures_util::stream::{self, BoxStream, Stream};
use serde::Serialize;
use serde_json::{Map, Value};
use trogon_aauth_verify::constants::SERVER_NONCE_HEADER;
use trogon_identity_types::SubjectPrincipal;
use trogon_identity_types::aauth::Requirement;
use trogon_identity_types::constants::REQUIREMENT;
use trogon_nats::{CredentialValue, NatsAuth, NatsConfig};
use trogon_std::secret_string::{EmptySecretError, SecretString};

use a2a_nats::RequestId;
use a2a_nats::constants::{GATEWAY_CALLER_ID_HEADER, GATEWAY_CALLER_ID_HTTP, REQ_ID_HEADER};
use a2a_nats::jetstream::consumers::{PullConfig, resubscribe_consumer, stream_events_consumer};
use a2a_nats::jetstream::streams::events_stream_name;
use a2a_nats::nats::subjects::gateway::GatewayIngressSubject;
use a2a_nats::{A2aMethod, A2aPrefix, A2aTaskId, ReqId};

use a2a_auth_callout::{CALLER_JWT_HEADER_NAME, CallerJwtHeaderValue, MintedUserJwt};

use crate::audit::{BridgedRequestRecord, RecordedCaller};
use crate::auth::AuthCalloutClient;
use crate::constants::{
    AGENT_ID_HEADER, INTERNAL_ERROR, JSONRPC_VERSION, SSE_EVENT_QUEUE_CAPACITY, WWW_AUTHENTICATE_BEARER,
    WWW_AUTHENTICATE_INVALID_BEARER,
};
use crate::edge_auth::{EdgeAdmission, EdgeGuard};
use crate::error::{BridgeError, EdgeAuthError};
use crate::identity::{BridgeAgentId, BridgeUserJwt, CallerHttpsAuth};
use crate::request::InboundRequest;

#[must_use]
pub fn default_a2a_prefix() -> A2aPrefix {
    // The literal "a2a" is the canonical bridge prefix and is checked by
    // a unit test below; if A2aPrefix ever changes its validator to
    // reject this value we'd rather catch it in CI than at runtime.
    A2aPrefix::new(String::from("a2a")).unwrap_or_else(|_| unreachable!("\"a2a\" is a valid A2aPrefix"))
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
    pub(crate) edge: Arc<dyn EdgeGuard>,
}

impl AppState {
    #[must_use]
    pub fn new(
        auth: Arc<dyn AuthCalloutClient>,
        publisher: Arc<dyn InboundGatewayPublish>,
        jetstream: Arc<dyn TaskJetStreamPort>,
        prefix: A2aPrefix,
        edge: Arc<dyn EdgeGuard>,
    ) -> Self {
        Self {
            auth,
            publisher,
            jetstream,
            prefix,
            edge,
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

/// The canned reply [`RecordingInboundPublisher`] hands back in place of a real
/// gateway response: a null id, since no request correlated it, and an empty
/// result.
#[derive(Serialize)]
struct StaticJsonOk {
    jsonrpc: &'static str,
    id: Option<Value>,
    result: serde_json::Map<String, Value>,
}

fn static_json_ok() -> Vec<u8> {
    let reply = StaticJsonOk {
        jsonrpc: JSONRPC_VERSION,
        id: None,
        result: serde_json::Map::new(),
    };
    serde_json::to_vec(&reply).unwrap_or_else(|_| b"{}".to_vec())
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

#[derive(Debug, thiserror::Error)]
enum CallerConnectError {
    #[error(transparent)]
    EmptyToken(#[from] EmptySecretError),
    #[error(transparent)]
    Refused(async_nats::ConnectError),
    #[error(transparent)]
    Unusable(trogon_nats::ConnectError),
}

impl From<trogon_nats::ConnectError> for CallerConnectError {
    fn from(error: trogon_nats::ConnectError) -> Self {
        match error {
            // The server list stays out of the message because it reaches HTTP callers.
            trogon_nats::ConnectError::ConnectionFailed { error, .. } => Self::Refused(error),
            other => Self::Unusable(other),
        }
    }
}

/// How long a per-request caller connection may take to come up. A distinct type
/// from [`GatewayRpcTimeout`] so a reply budget cannot be passed where a connect
/// budget belongs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CallerConnectTimeout(Duration);

impl CallerConnectTimeout {
    #[must_use]
    pub const fn new(timeout: Duration) -> Self {
        Self(timeout)
    }

    #[must_use]
    pub const fn get(self) -> Duration {
        self.0
    }
}

/// How long a gateway request may wait for its reply once the caller connection is up.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GatewayRpcTimeout(Duration);

impl GatewayRpcTimeout {
    #[must_use]
    pub const fn new(timeout: Duration) -> Self {
        Self(timeout)
    }

    #[must_use]
    pub const fn get(self) -> Duration {
        self.0
    }
}

async fn connect_as_caller(
    nats: &NatsConfig,
    caller_jwt: &BridgeUserJwt,
    timeout: CallerConnectTimeout,
) -> Result<async_nats::Client, CallerConnectError> {
    let token = CredentialValue::Literal(SecretString::new(caller_jwt.as_str())?);
    Ok(trogon_nats::connect_once(nats, NatsAuth::Token(token), timeout.get()).await?)
}

#[derive(Clone)]
pub struct AsyncNatsTokenGatewayUnary {
    nats: Arc<NatsConfig>,
    connect_timeout: CallerConnectTimeout,
    rpc_timeout: GatewayRpcTimeout,
}

impl AsyncNatsTokenGatewayUnary {
    #[must_use]
    pub fn new(nats: NatsConfig, connect_timeout: CallerConnectTimeout, rpc_timeout: GatewayRpcTimeout) -> Self {
        Self {
            nats: Arc::new(nats),
            connect_timeout,
            rpc_timeout,
        }
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
        let client = connect_as_caller(&self.nats, caller_jwt, self.connect_timeout)
            .await
            .map_err(|e| BridgeError::NatsPublish(e.to_string()))?;
        // request_with_headers itself has no deadline — without this
        // wrapper a hung gateway responder blocks the inbound HTTPS
        // request indefinitely and the configured RPC timeout is silently
        // ignored.
        tokio::time::timeout(
            self.rpc_timeout.get(),
            client.request_with_headers(subject.to_owned(), headers, payload),
        )
        .await
        .map_err(|_| BridgeError::NatsPublish(format!("gateway RPC exceeded {:?}", self.rpc_timeout.get())))?
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
    /// A live subscription: the task is shared, so `req_id` is what separates this
    /// caller's events from those of anyone else streaming the same task (ADR#0055).
    MessageStreamBootstrap { task_id: A2aTaskId, req_id: ReqId },
    /// A resume: the events being replayed carry the `req_id` of the original
    /// subscription, not of this request, so there is nothing to demux against.
    TasksResubscribe { task_id: A2aTaskId, last_seq: u64 },
}

impl SseConsumePlan {
    /// The consumer to open on the caller's own task events, paired with the
    /// correlation id an event must carry to reach this caller.
    fn consumer(&self, prefix: &A2aPrefix, caller: &SubjectPrincipal) -> (PullConfig, Option<ReqId>) {
        match self {
            SseConsumePlan::MessageStreamBootstrap { task_id, req_id } => {
                (stream_events_consumer(prefix, task_id, caller), Some(req_id.clone()))
            }
            SseConsumePlan::TasksResubscribe { task_id, last_seq } => {
                (resubscribe_consumer(prefix, task_id, caller, *last_seq), None)
            }
        }
    }
}

/// Whether an event read off the task-events stream belongs on this SSE response.
///
/// `demux` is `None` for a resume, which is entitled to the whole replay. For a live
/// subscription it is the request's own id, and anything the agent stamped for another
/// subscription to the same task is left out.
fn event_forwards_to_caller(demux: Option<&ReqId>, headers: Option<&async_nats::HeaderMap>) -> bool {
    demux.is_none_or(|req_id| req_id.matches_event_headers(headers))
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
    nats: Arc<NatsConfig>,
    connect_timeout: CallerConnectTimeout,
}

impl AsyncNatsTokenTaskJetstream {
    #[must_use]
    pub fn new(nats: NatsConfig, connect_timeout: CallerConnectTimeout) -> Self {
        Self {
            nats: Arc::new(nats),
            connect_timeout,
        }
    }

    async fn connect_caller_client(&self, jwt: &BridgeUserJwt) -> Result<async_nats::Client, BridgeError> {
        connect_as_caller(&self.nats, jwt, self.connect_timeout)
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
        let caller = caller_jwt.subject_principal()?;
        let client = self.connect_caller_client(caller_jwt).await?;
        let jetstream_ctx = async_nats::jetstream::new(client);
        let stream_name = events_stream_name(prefix);
        let stream = jetstream_ctx
            .get_stream(stream_name.clone())
            .await
            .map_err(|e| BridgeError::JetStreamConsume(format!("get_stream {stream_name}: {e}")))?;
        let (pull_cfg, demux_req_id) = plan.consumer(prefix, &caller);
        let consumer = stream
            .create_consumer(pull_cfg)
            .await
            .map_err(|e| BridgeError::JetStreamConsume(format!("create_consumer: {e}")))?;

        let mut messages_stream = consumer
            .messages()
            .await
            .map_err(|e| BridgeError::JetStreamConsume(format!("consumer.messages: {e}")))?;

        let (tx, rx) = tokio::sync::mpsc::channel(SSE_EVENT_QUEUE_CAPACITY);

        let handle = tokio::spawn(async move {
            while let Some(item) = messages_stream.next().await {
                match item {
                    Ok(js_message) => {
                        // Enqueue BEFORE acking — the send waits for the
                        // reader to make room, so a slow SSE client holds the
                        // ack back and JetStream keeps the backlog. If the
                        // client already disconnected the receiver is closed
                        // and we want JetStream to redeliver this event
                        // to the next consumer rather than dropping it.
                        // An event belonging to another subscription to the same task
                        // is acked without being enqueued, so it leaves the stream
                        // without reaching this caller.
                        if event_forwards_to_caller(demux_req_id.as_ref(), js_message.message.headers.as_ref())
                            && tx.send(Ok(Bytes::clone(&js_message.message.payload))).await.is_err()
                        {
                            break;
                        }
                        if let Err(ack_err) = js_message.ack().await {
                            let _ = tx.send(Err(BridgeError::JetStreamConsume(ack_err.to_string()))).await;
                            break;
                        }
                    }
                    Err(err) => {
                        let _ = tx.send(Err(BridgeError::JetStreamConsume(err.to_string()))).await;
                        break;
                    }
                }
            }
        });

        Ok(Box::pin(RxPollStream {
            rx,
            handle: Some(handle),
        }))
    }
}

struct RxPollStream {
    rx: tokio::sync::mpsc::Receiver<Result<Bytes, BridgeError>>,
    handle: Option<tokio::task::JoinHandle<()>>,
}

impl Stream for RxPollStream {
    type Item = Result<Bytes, BridgeError>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.rx.poll_recv(cx)
    }
}

impl Drop for RxPollStream {
    fn drop(&mut self) {
        // Cancel the JetStream pull task when the SSE stream is dropped
        // (HTTPS client disconnect, axum tearing down the response,
        // etc.) — otherwise the task can sit blocked on the next
        // message and keep the NATS connection + ephemeral consumer
        // alive until JetStream times out.
        if let Some(handle) = self.handle.take() {
            handle.abort();
        }
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
                return Err(BridgeError::JetStreamConsume("scripted mutex poisoned".into()));
            };
            std::mem::take(&mut (*guard))
        };
        Ok(Box::pin(stream::iter(drained)))
    }
}

/// Every A2A SSE frame is an unnamed `data:` line carrying a JSON-RPC response
/// that repeats the caller's request id. Naming the events instead would put the
/// bootstrap and the task chunks on distinct SSE event types, which a spec client
/// never subscribes to, and the bodies already say which is which.
///
/// The id is restamped rather than forwarded, because the hops behind this edge
/// correlate on their own transport id (`Trogon-Req-Id`, or a minted one when the
/// caller sent no id at all). Only the caller's own id is meaningful to the
/// caller. A payload that is not a JSON-RPC response leaves as a correlated
/// error frame: a client parses every `data:` line as a JSON-RPC response, so
/// anything else there is a parse failure rather than the diagnostic it looks
/// like.
///
/// An event an older agent published carries no envelope of its own, so it is
/// given one here instead of being restamped: forwarding it as-is would put a
/// `data:` line on the wire that no spec client can parse as a response.
fn sse_data_line(body: &[u8], caller_id: &Value, record: &BridgedRequestRecord) -> Event {
    if let Some(response) = a2a_nats::task_event::legacy_event_as_response(body, caller_id) {
        return Event::default().data(response.to_string());
    }

    let Some(mut envelope) = serde_json::from_slice::<Value>(body).ok().and_then(response_envelope) else {
        record.undecodable_stream_payload(body);
        return sse_error_frame(caller_id, "stream payload is not a JSON-RPC envelope".to_owned());
    };
    envelope.insert("id".to_owned(), caller_id.clone());
    Event::default().data(Value::Object(envelope).to_string())
}

/// A JSON-RPC response carries exactly one of `result` and `error`. An object
/// holding neither is a request, an empty body, or something else entirely, and
/// stamping the caller's id onto it would emit a `data:` line that answers
/// nothing.
fn response_envelope(body: Value) -> Option<Map<String, Value>> {
    let Value::Object(envelope) = body else {
        return None;
    };
    (envelope.contains_key("result") != envelope.contains_key("error")).then_some(envelope)
}

fn sse_error_line(caller_id: &Value, err: &BridgeError) -> Event {
    sse_error_frame(caller_id, err.to_string())
}

/// The `error` member of a JSON-RPC response.
#[derive(Serialize)]
struct SseErrorDetail {
    code: i32,
    message: String,
}

/// JSON-RPC error response carried on an SSE frame, echoing the caller's id so
/// the failure correlates with the request that opened the stream.
#[derive(Serialize)]
struct SseErrorEnvelope<'a> {
    jsonrpc: &'static str,
    id: &'a Value,
    error: SseErrorDetail,
}

fn sse_error_frame(caller_id: &Value, message: String) -> Event {
    let envelope = SseErrorEnvelope {
        jsonrpc: JSONRPC_VERSION,
        id: caller_id,
        error: SseErrorDetail {
            code: INTERNAL_ERROR,
            message,
        },
    };
    Event::default().data(serde_json::to_string(&envelope).unwrap_or_default())
}

fn sse_from_bootstrap_and_payloads(
    bootstrap_owned: Vec<u8>,
    tail: Pin<Box<dyn Stream<Item = Result<Bytes, BridgeError>> + Send>>,
    caller_id: Value,
    record: BridgedRequestRecord,
) -> BoxStream<'static, Result<Event, Infallible>> {
    let head_event = sse_data_line(&bootstrap_owned, &caller_id, &record);
    let head = futures_util::stream::once(futures_util::future::ready(Ok::<Event, Infallible>(head_event)));
    let tail_mapped = tail.map(move |item| {
        Ok::<Event, Infallible>(match item {
            Ok(chunk) => sse_data_line(chunk.as_ref(), &caller_id, &record),
            Err(ref err) => sse_error_line(&caller_id, err),
        })
    });

    Box::pin(head.chain(tail_mapped))
}

fn gateway_req_headers(correlation: ReqId, caller_id: Option<&str>) -> Result<async_nats::HeaderMap, BridgeError> {
    let mut map = async_nats::HeaderMap::new();
    let value = correlation.as_str();
    map.insert(REQ_ID_HEADER, async_nats::header::HeaderValue::from(value));
    // Forward the external caller id (HTTP `x-a2a-caller-id`) onto the
    // NATS request as `X-A2a-Caller-Id` so gateway audits, spans, and
    // caller-scoped routing can see who called even though the bridge
    // itself authenticates with the minted user JWT.
    if let Some(raw) = caller_id {
        let name = async_nats::header::HeaderName::from_static(GATEWAY_CALLER_ID_HEADER);
        map.insert(name, async_nats::header::HeaderValue::from(raw));
    }
    Ok(map)
}

/// Gateway publish headers propagate JSON-RPC correlation, optional external caller id,
/// and the auth-callout minted user JWT.
pub fn gateway_publish_headers(
    correlation: ReqId,
    caller_jwt: &BridgeUserJwt,
    caller_id: Option<&str>,
) -> Result<async_nats::HeaderMap, BridgeError> {
    let mut map = gateway_req_headers(correlation, caller_id)?;
    let minted = MintedUserJwt::new(caller_jwt.as_str()).map_err(|e| BridgeError::Mint(e.to_string()))?;
    let header_value = CallerJwtHeaderValue::from_minted(&minted);
    let nats_name = async_nats::header::HeaderName::from_static(CALLER_JWT_HEADER_NAME);
    map.insert(nats_name, async_nats::header::HeaderValue::from(header_value.as_str()));
    Ok(map)
}

fn caller_auth_from(headers: &HeaderMap) -> Result<CallerHttpsAuth, BridgeError> {
    let raw = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .ok_or(BridgeError::MissingAuthorization)?;
    Ok(CallerHttpsAuth::new(raw.to_owned()))
}

fn caller_id_from_headers(headers: &HeaderMap) -> Option<String> {
    headers
        .get(GATEWAY_CALLER_ID_HTTP)
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
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
    // Direct cursor keys — `lastSeq` is the canonical name used by
    // `a2a-nats-http` and `a2a-nats-stdio`; the others are
    // backwards-compatible aliases the SDK has accepted over time.
    for key in [
        "lastSeq",
        "last_known_sequence_number",
        "lastSequence",
        "last_sequence",
        "last_seq",
        "resume_from_sequence",
        "resumeFromSeq",
    ] {
        if let Some(parsed) = read_u64_field(params, key) {
            return Some(parsed);
        }
    }
    // Older clients pass the resume point under SSE-style
    // `metadata.lastEventId`; honor it so reconnects don't replay or
    // skip events for those callers.
    if let Some(metadata) = params.get("metadata") {
        for key in ["lastEventId", "last_event_id"] {
            if let Some(parsed) = read_u64_field(metadata, key) {
                return Some(parsed);
            }
        }
    }
    None
}

fn read_u64_field(value: &Value, key: &str) -> Option<u64> {
    let field = value.get(key)?;
    if let Some(n) = field.as_u64() {
        return Some(n);
    }
    field.as_str()?.parse::<u64>().ok()
}

fn resub_task_and_seq(body: &Value) -> Result<(A2aTaskId, u64), BridgeError> {
    let params = body
        .get("params")
        .ok_or_else(|| BridgeError::StreamingParams("tasks/resubscribe expects params envelope".into()))?;
    let tid = params
        .get("id")
        .and_then(Value::as_str)
        .or_else(|| params.get("task_id").and_then(Value::as_str))
        .or_else(|| params.get("taskId").and_then(Value::as_str))
        .ok_or_else(|| BridgeError::StreamingParams("tasks/resubscribe missing task identifier".into()))?;
    let task_id = A2aTaskId::new(tid).map_err(|e| BridgeError::StreamingParams(e.to_string()))?;
    Ok((task_id, extract_last_sequence(params).unwrap_or(0)))
}

/// The consumer plan for a streaming method, or `None` when no events can follow.
///
/// `message/stream` may answer with a bare `Message` instead of a `Task`. There is no
/// task to scope a consumer to and nothing will be published, so the caller gets the
/// bootstrap line alone rather than a consumer on a subject nobody writes.
fn sse_plan(
    method: &str,
    body: &Value,
    bootstrap_reply: &[u8],
    req_id: &ReqId,
) -> Result<Option<SseConsumePlan>, BridgeError> {
    match method {
        "message/stream" => {
            Ok(
                bootstrap_task_id(bootstrap_reply)?.map(|task_id| SseConsumePlan::MessageStreamBootstrap {
                    task_id,
                    req_id: req_id.clone(),
                }),
            )
        }
        "tasks/resubscribe" => {
            let (task_id, last_seq) = resub_task_and_seq(body)?;
            Ok(Some(SseConsumePlan::TasksResubscribe { task_id, last_seq }))
        }
        other => Err(BridgeError::StreamingParams(format!(
            "unsupported streaming method {other}"
        ))),
    }
}

/// The task id carried by a `message/stream` bootstrap reply, if it carries one.
///
/// `SendMessageResponse` serializes as a single-key map naming the variant, so a
/// task-shaped reply nests the task one level under `result`. A `Message`-shaped
/// reply has no `task` key at all, which is the `None` that tells the caller not
/// to open a consumer.
fn bootstrap_task_id(reply: &[u8]) -> Result<Option<A2aTaskId>, BridgeError> {
    let envelope: Value = serde_json::from_slice(reply).map_err(BridgeError::Deserialize)?;
    let Some(raw) = envelope.pointer("/result/task/id").and_then(Value::as_str) else {
        return Ok(None);
    };
    A2aTaskId::new(raw)
        .map(Some)
        .map_err(|e| BridgeError::StreamingParams(format!("agent returned an invalid task id: {e}")))
}

/// True when the gateway unary reply is a well-formed JSON-RPC error
/// envelope. Streaming and unary paths both forward this envelope as
/// HTTP 200 with the JSON-RPC body intact (mirroring `a2a-nats-http`)
/// rather than escalating it to HTTP 502 at the bridge layer.
fn gateway_reply_is_jsonrpc_error(slice: &[u8]) -> bool {
    serde_json::from_slice::<Value>(slice)
        .map(|v| v.get("error").is_some())
        .unwrap_or(false)
}

pub fn gateway_router(state: AppState) -> Router {
    Router::new().route("/", post(a2a_post)).with_state(state)
}

async fn a2a_post(
    State(state): State<AppState>,
    method: Method,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    match handle_jsonrpc(InboundRequest::new(method, uri, headers, body), &state).await {
        Ok(r) => r,
        Err(e) => bridge_error_into_response(e),
    }
}

/// Failure body for a request that never reached the gateway, so there is no
/// JSON-RPC envelope to carry the error in.
#[derive(Serialize)]
struct BridgeErrorBody {
    error: String,
}

fn bridge_error_into_response(e: BridgeError) -> Response {
    let payload = axum::Json(BridgeErrorBody { error: e.to_string() });
    match &e {
        BridgeError::MissingAuthorization => (
            StatusCode::UNAUTHORIZED,
            [(WWW_AUTHENTICATE, WWW_AUTHENTICATE_BEARER.to_owned())],
            payload,
        )
            .into_response(),
        BridgeError::EdgeAuth(error) => edge_refusal_response(error, payload),
        _ => (StatusCode::BAD_GATEWAY, payload).into_response(),
    }
}

/// Tells a refused caller what to fix: sign with an agent token, bind the
/// nonce it is handed, present a bearer naming the caller that signed, or
/// retry once the edge can check proofs again.
fn edge_refusal_response(error: &EdgeAuthError, payload: axum::Json<BridgeErrorBody>) -> Response {
    match error {
        EdgeAuthError::NonceRequired(challenge) => (
            StatusCode::UNAUTHORIZED,
            [(SERVER_NONCE_HEADER, challenge.nonce().as_str().to_owned())],
            payload,
        )
            .into_response(),
        EdgeAuthError::Unavailable(_) => (StatusCode::SERVICE_UNAVAILABLE, payload).into_response(),
        EdgeAuthError::Unsigned(_) | EdgeAuthError::Unverified(_) | EdgeAuthError::UnidentifiedPresenter(_) => (
            StatusCode::UNAUTHORIZED,
            [(REQUIREMENT, Requirement::AgentToken.to_header_value())],
            payload,
        )
            .into_response(),
        EdgeAuthError::BearerMismatch { .. } | EdgeAuthError::UnidentifiedBearer(_) => (
            StatusCode::UNAUTHORIZED,
            [(WWW_AUTHENTICATE, WWW_AUTHENTICATE_INVALID_BEARER.to_owned())],
            payload,
        )
            .into_response(),
    }
}

/// Holds the request to the edge's proof of possession before anything is
/// minted for it, recording every refusal and every proof shadow mode lets
/// through.
async fn admit_at_edge(
    state: &AppState,
    request: &InboundRequest,
    caller: &CallerHttpsAuth,
    req_id: &ReqId,
) -> Result<EdgeAdmission, EdgeAuthError> {
    match state.edge.admit(request, caller).await {
        Ok(EdgeAdmission::Shadowed(error)) => {
            BridgedRequestRecord::new(RecordedCaller::behind(&error), req_id.clone())
                .shadowed_at_edge(state.edge.mode(), &error);
            Ok(EdgeAdmission::Shadowed(error))
        }
        Ok(admission) => Ok(admission),
        Err(error) => {
            BridgedRequestRecord::new(RecordedCaller::behind(&error), req_id.clone())
                .refused_at_edge(state.edge.mode(), &error);
            Err(error)
        }
    }
}

pub async fn handle_jsonrpc(request: InboundRequest, state: &AppState) -> Result<Response, BridgeError> {
    let caller_auth = caller_auth_from(request.headers())?;
    let agent_id = agent_header_parse(request.headers())?;
    let caller_id = caller_id_from_headers(request.headers());
    let body = request.body();
    let rpc = serde_json::from_slice::<Value>(body);
    // Computed once per request, before the edge decides on it, so the edge's
    // records and every hop behind the bridge name the same id. When the
    // JSON-RPC id is absent or null, or the body is not JSON, this mints a fresh
    // one, so computing it twice would publish under another id than the one
    // the records report.
    let req_id = rpc.as_ref().map(json_rpc_corr_id).unwrap_or_default();
    admit_at_edge(state, &request, &caller_auth, &req_id).await?;
    let jwt = state.auth.mint(&caller_auth).await?;
    let caller = jwt.subject_principal()?;
    let record = BridgedRequestRecord::new(RecordedCaller::SubjectPrincipal(caller.clone()), req_id);
    let v = rpc.map_err(BridgeError::Deserialize)?;
    let Some(method) = v.get("method").and_then(Value::as_str) else {
        return Err(BridgeError::MissingJsonRpcMethod);
    };
    let a2a_method = A2aMethod::from_jsonrpc_name(method).ok_or(BridgeError::UnknownJsonRpcMethod)?;
    let subject = GatewayIngressSubject::new(&state.prefix, agent_id.as_agent_id(), &caller, a2a_method).to_string();

    if is_sse_jsonrpc_method(method) {
        // Every SSE frame repeats this id, so a streaming request that carries
        // none has no reply the caller can correlate. Rejecting here keeps the
        // gateway from doing the work of a stream nobody can read.
        let stream_caller_id = v
            .get("id")
            .and_then(|id| serde_json::from_value::<RequestId>(id.clone()).ok())
            .ok_or(BridgeError::MissingJsonRpcId)?;
        // The unary publish comes BEFORE the JetStream consumer. Task event
        // subjects are scoped to the task (ADR#0055), and the bootstrap reply is
        // where the task id comes from. Nothing is lost in the gap, because
        // `A2A_EVENTS` retains by limits and the consumer delivers from the start
        // of the stream.
        let nats_headers = gateway_publish_headers(record.req_id().clone(), &jwt, caller_id.as_deref())?;
        record.presented_to_gateway();
        let unary_reply = state
            .publisher
            .publish_unary_to_gateway(&subject, &jwt, nats_headers, body.as_ref())
            .await?;
        if gateway_reply_is_jsonrpc_error(&unary_reply) {
            // Match the unary path / a2a-nats-http: a JSON-RPC error
            // from the gateway is the caller's failure to read in the
            // returned envelope, not a bridge-layer transport failure.
            return Response::builder()
                .status(StatusCode::OK)
                .header(axum::http::header::CONTENT_TYPE, "application/json")
                .body(axum::body::Body::from(unary_reply))
                .map_err(|e| BridgeError::ResponseBuild(e.to_string()));
        }
        let payloads = match sse_plan(method, &v, &unary_reply, record.req_id())? {
            Some(plan) => {
                state
                    .jetstream
                    .task_event_payload_stream(&jwt, &state.prefix, plan)
                    .await?
            }
            None => Box::pin(futures_util::stream::empty()),
        };
        let merged =
            sse_from_bootstrap_and_payloads(unary_reply.to_vec(), payloads, stream_caller_id.to_json(), record);
        return Ok(Sse::new(merged).keep_alive(KeepAlive::default()).into_response());
    }

    let nats_headers = gateway_publish_headers(record.req_id().clone(), &jwt, caller_id.as_deref())?;
    record.presented_to_gateway();
    let reply = state
        .publisher
        .publish_unary_to_gateway(&subject, &jwt, nats_headers, body.as_ref())
        .await?;
    Response::builder()
        .status(StatusCode::OK)
        .header(axum::http::header::CONTENT_TYPE, "application/json")
        .body(axum::body::Body::from(reply))
        .map_err(|e| BridgeError::ResponseBuild(e.to_string()))
}

#[cfg(test)]
mod tests;
