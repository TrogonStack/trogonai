use crate::acp_connection_id::AcpConnectionId;
use crate::connection;
use crate::constants::{
    ACP_CONNECTION_ID_HEADER, ACP_PROTOCOL_VERSION_HEADER, ACP_SESSION_ID_HEADER, HTTP_CHANNEL_CAPACITY,
    X_ACCEL_BUFFERING_HEADER,
};
use acp_nats::{agent::Bridge, client, spawn_notification_forwarder};
use agent_client_protocol::{AgentSideConnection, ProtocolVersion, RequestId, SessionNotification};
use axum::extract::FromRequestParts;
use axum::extract::Request;
use axum::extract::State;
use axum::extract::ws::{WebSocket, WebSocketUpgrade};
use axum::http::header::{ACCEPT, CONTENT_TYPE, HOST, ORIGIN};
use axum::http::{HeaderMap, HeaderValue, StatusCode, Uri, uri::Authority};
use axum::response::sse::{Event, Sse};
use axum::response::{IntoResponse, Response};
use futures_util::stream;
use serde::Deserialize;
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::convert::Infallible;
use std::net::IpAddr;
use std::rc::Rc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
use tokio::sync::{mpsc, oneshot, watch};
use tracing::{error as tracing_error, info, warn};
use trogon_std::time::SystemClock;
use uuid::Uuid;

type BoxError = Box<dyn std::error::Error + Send>;
type SseSender = mpsc::Sender<SseFrame>;
type SseReceiver = mpsc::Receiver<SseFrame>;

#[derive(Clone)]
pub struct AppState {
    pub bind_host: IpAddr,
    pub manager_tx: mpsc::UnboundedSender<ManagerRequest>,
    pub shutdown_tx: watch::Sender<bool>,
}

pub struct ConnectionRequest {
    pub connection_id: AcpConnectionId,
    pub socket: WebSocket,
    pub shutdown_rx: watch::Receiver<bool>,
}

pub enum ManagerRequest {
    WebSocket(Box<ConnectionRequest>),
    HttpPost {
        connection_id: Option<AcpConnectionId>,
        protocol_version: Option<ProtocolVersion>,
        session_id: Option<acp_nats::AcpSessionId>,
        message: IncomingHttpMessage,
        response: oneshot::Sender<Result<HttpPostOutcome, HttpTransportError>>,
        shutdown_rx: watch::Receiver<bool>,
    },
    HttpGet {
        connection_id: AcpConnectionId,
        protocol_version: Option<ProtocolVersion>,
        response: oneshot::Sender<Result<HttpGetOutcome, HttpTransportError>>,
    },
    HttpDelete {
        connection_id: AcpConnectionId,
        protocol_version: Option<ProtocolVersion>,
        response: oneshot::Sender<Result<Option<ProtocolVersion>, HttpTransportError>>,
    },
}

#[derive(Debug)]
pub enum HttpPostOutcome {
    Accepted,
    Json {
        connection_id: AcpConnectionId,
        protocol_version: Option<ProtocolVersion>,
        body: String,
    },
}

#[derive(Debug)]
pub struct HttpGetOutcome {
    pub protocol_version: Option<ProtocolVersion>,
    pub stream: SseReceiver,
}

#[derive(Debug, thiserror::Error)]
pub enum HttpTransportError {
    #[error("{message}")]
    BadRequest {
        message: &'static str,
        #[source]
        source: Option<BoxError>,
    },
    #[error("{message}")]
    NotFound {
        message: &'static str,
        #[source]
        source: Option<BoxError>,
    },
    #[error("{message}")]
    Conflict {
        message: &'static str,
        #[source]
        source: Option<BoxError>,
    },
    #[error("{message}")]
    Forbidden {
        message: &'static str,
        #[source]
        source: Option<BoxError>,
    },
    #[error("{message}")]
    UnsupportedMediaType {
        message: &'static str,
        #[source]
        source: Option<BoxError>,
    },
    #[error("{message}")]
    NotAcceptable {
        message: &'static str,
        #[source]
        source: Option<BoxError>,
    },
    #[error("{message}")]
    NotImplemented {
        message: &'static str,
        #[source]
        source: Option<BoxError>,
    },
    #[error("{message}")]
    Internal {
        message: &'static str,
        #[source]
        source: Option<BoxError>,
    },
}

impl HttpTransportError {
    fn bad_request(message: &'static str) -> Self {
        Self::BadRequest { message, source: None }
    }

    fn bad_request_with(message: &'static str, source: impl std::error::Error + Send + 'static) -> Self {
        Self::BadRequest {
            message,
            source: Some(Box::new(source)),
        }
    }

    fn not_found(message: &'static str) -> Self {
        Self::NotFound { message, source: None }
    }

    fn conflict(message: &'static str) -> Self {
        Self::Conflict { message, source: None }
    }

    fn forbidden(message: &'static str) -> Self {
        Self::Forbidden { message, source: None }
    }

    fn forbidden_with(message: &'static str, source: impl std::error::Error + Send + 'static) -> Self {
        Self::Forbidden {
            message,
            source: Some(Box::new(source)),
        }
    }

    fn unsupported_media_type(message: &'static str) -> Self {
        Self::UnsupportedMediaType { message, source: None }
    }

    fn not_acceptable(message: &'static str) -> Self {
        Self::NotAcceptable { message, source: None }
    }

    fn not_implemented(message: &'static str) -> Self {
        Self::NotImplemented { message, source: None }
    }

    fn internal(message: &'static str) -> Self {
        Self::Internal { message, source: None }
    }

    fn internal_with(message: &'static str, source: impl std::error::Error + Send + 'static) -> Self {
        Self::Internal {
            message,
            source: Some(Box::new(source)),
        }
    }

    fn status_code(&self) -> StatusCode {
        match self {
            Self::BadRequest { .. } => StatusCode::BAD_REQUEST,
            Self::NotFound { .. } => StatusCode::NOT_FOUND,
            Self::Conflict { .. } => StatusCode::CONFLICT,
            Self::Forbidden { .. } => StatusCode::FORBIDDEN,
            Self::UnsupportedMediaType { .. } => StatusCode::UNSUPPORTED_MEDIA_TYPE,
            Self::NotAcceptable { .. } => StatusCode::NOT_ACCEPTABLE,
            Self::NotImplemented { .. } => StatusCode::NOT_IMPLEMENTED,
            Self::Internal { .. } => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    fn into_response(self) -> Response {
        let status = self.status_code();
        (status, self.to_string()).into_response()
    }
}

#[derive(Debug)]
pub struct HttpConnectionHandle {
    pub command_tx: mpsc::UnboundedSender<HttpConnectionCommand>,
}

#[derive(Debug)]
pub enum HttpConnectionCommand {
    Post {
        protocol_version: Option<ProtocolVersion>,
        session_id: Option<acp_nats::AcpSessionId>,
        message: IncomingHttpMessage,
        response: oneshot::Sender<Result<HttpPostOutcome, HttpTransportError>>,
    },
    AttachListener {
        protocol_version: Option<ProtocolVersion>,
        response: oneshot::Sender<Result<HttpGetOutcome, HttpTransportError>>,
    },
    Close {
        protocol_version: Option<ProtocolVersion>,
        response: oneshot::Sender<Result<Option<ProtocolVersion>, HttpTransportError>>,
    },
}

#[derive(Clone, Debug)]
pub(crate) enum SseFrame {
    Json { event_id: Option<String>, json: String },
}

impl SseFrame {
    fn json(json: String) -> Self {
        Self::Json {
            event_id: Some(Uuid::new_v4().to_string()),
            json,
        }
    }

    fn into_event(self) -> Event {
        match self {
            Self::Json { event_id, json } => {
                let event = Event::default().data(json);
                if let Some(event_id) = event_id {
                    event.id(event_id)
                } else {
                    event
                }
            }
        }
    }
}

#[derive(Debug)]
enum PendingRequest {
    Initialize {
        request_id: RequestId,
        response: oneshot::Sender<Result<HttpPostOutcome, HttpTransportError>>,
    },
}

fn fail_pending_initialize_on_close(pending_request: &mut Option<PendingRequest>) {
    if let Some(PendingRequest::Initialize { response, .. }) = pending_request.take() {
        let _ = response.send(Err(HttpTransportError::internal(
            "HTTP connection closed before the request completed",
        )));
    }
}

#[derive(Clone, Debug)]
struct PendingResponseContext {
    session_id: acp_nats::AcpSessionId,
    activate_session_on_success: bool,
    inject_session_id: bool,
}

#[derive(Debug, Deserialize)]
pub struct IncomingHttpMessage {
    pub id: Option<RequestId>,
    pub method: Option<String>,
    pub params: Option<Value>,
    #[serde(skip)]
    pub raw: String,
    #[serde(skip)]
    has_result: bool,
    #[serde(skip)]
    has_error: bool,
}

impl IncomingHttpMessage {
    pub fn parse(raw: String) -> Result<Self, HttpTransportError> {
        let mut messages = Self::parse_all(raw)?;
        if messages.len() != 1 {
            return Err(HttpTransportError::bad_request(
                "expected a single JSON-RPC message; use parse_all for batch arrays",
            ));
        }
        Ok(messages.remove(0))
    }

    /// Parse one JSON-RPC message or unbundle a batch array into individual messages.
    pub fn parse_all(raw: String) -> Result<Vec<Self>, HttpTransportError> {
        let trimmed = raw.trim_start();
        if trimmed.starts_with('[') {
            let values = serde_json::from_str::<Vec<Value>>(&raw)
                .map_err(|error| HttpTransportError::bad_request_with("invalid JSON-RPC payload", error))?;
            if values.is_empty() {
                return Err(HttpTransportError::bad_request("empty JSON-RPC batch"));
            }
            values
                .into_iter()
                .map(|value| {
                    let element_raw = serde_json::to_string(&value).map_err(|error| {
                        HttpTransportError::bad_request_with("invalid JSON-RPC payload", error)
                    })?;
                    Self::parse_one(element_raw)
                })
                .collect()
        } else {
            Ok(vec![Self::parse_one(raw)?])
        }
    }

    fn parse_one(raw: String) -> Result<Self, HttpTransportError> {
        let value = serde_json::from_str::<Value>(&raw)
            .map_err(|error| HttpTransportError::bad_request_with("invalid JSON-RPC payload", error))?;
        let (has_result, has_error) = value
            .as_object()
            .map(|object| (object.contains_key("result"), object.contains_key("error")))
            .unwrap_or((false, false));
        let mut parsed = serde_json::from_value::<Self>(value)
            .map_err(|error| HttpTransportError::bad_request_with("invalid JSON-RPC payload", error))?;
        parsed.raw = raw;
        parsed.has_result = has_result;
        parsed.has_error = has_error;
        Ok(parsed)
    }

    fn is_request(&self) -> bool {
        self.id.is_some() && self.method.is_some()
    }

    fn is_notification(&self) -> bool {
        self.id.is_none() && self.method.is_some()
    }

    fn is_response(&self) -> bool {
        self.id.is_some() && self.method.is_none() && (self.has_result || self.has_error)
    }

    fn method_name(&self) -> Option<&str> {
        self.method.as_deref()
    }

    fn is_initialize(&self) -> bool {
        self.method_name() == Some("initialize")
    }

    fn creates_session(&self) -> bool {
        matches!(self.method_name(), Some("session/new") | Some("session/fork"))
    }

    fn attaches_session(&self) -> bool {
        matches!(self.method_name(), Some("session/load") | Some("session/resume"))
    }

    fn allows_unknown_session(&self) -> bool {
        self.creates_session() || self.attaches_session()
    }

    fn requires_session_id(&self) -> bool {
        if self.is_response() {
            return true;
        }

        match self.method_name() {
            Some("initialize") | Some("authenticate") | Some("session/list") | Some("session/new") => false,
            Some(method) if method.starts_with("session/") => true,
            _ => false,
        }
    }

    fn params_session_id(&self) -> Result<Option<acp_nats::AcpSessionId>, HttpTransportError> {
        let Some(params) = &self.params else {
            return Ok(None);
        };

        let Some(session_id) = params.get("sessionId").and_then(Value::as_str) else {
            return Ok(None);
        };

        acp_nats::AcpSessionId::new(session_id)
            .map(Some)
            .map_err(|error| HttpTransportError::bad_request_with("invalid sessionId in request body", error))
    }
}

#[derive(Debug)]
struct OutgoingHttpMessage {
    id: Option<RequestId>,
    params: Option<Value>,
    result: Option<Value>,
    has_error: bool,
}

impl OutgoingHttpMessage {
    fn parse(raw: &str) -> Option<Self> {
        let value: Value = serde_json::from_str(raw).ok()?;
        let object = value.as_object()?;
        Some(Self {
            id: object.get("id").and_then(|id| serde_json::from_value(id.clone()).ok()),
            params: object.get("params").cloned(),
            result: object.get("result").cloned(),
            has_error: object.contains_key("error"),
        })
    }

    fn is_response(&self) -> bool {
        self.id.is_some() && self.params.is_none()
    }

    fn is_success_response(&self) -> bool {
        self.is_response() && !self.has_error
    }

    fn result_session_id(&self) -> Option<acp_nats::AcpSessionId> {
        let result = self.result.as_ref()?;
        let session_id = result.get("sessionId")?.as_str()?;
        acp_nats::AcpSessionId::new(session_id).ok()
    }

    fn result_protocol_version(&self) -> Option<ProtocolVersion> {
        let result = self.result.as_ref()?;
        let protocol_version = result.get("protocolVersion")?;
        serde_json::from_value(protocol_version.clone()).ok()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ListenerDispatch {
    Missing,
    Delivered,
    Dropped,
}

fn dispatch_to_get_listeners(frame: &SseFrame, get_listeners: &mut Vec<SseSender>) -> ListenerDispatch {
    if get_listeners.is_empty() {
        return ListenerDispatch::Missing;
    }

    let mut delivered = false;
    let mut retained = Vec::with_capacity(get_listeners.len());

    for listener in std::mem::take(get_listeners) {
        match listener.try_send(frame.clone()) {
            Ok(()) => {
                delivered = true;
                retained.push(listener);
            }
            Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {
                warn!("Dropping stalled HTTP SSE listener");
            }
            Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {}
        }
    }

    *get_listeners = retained;

    if delivered {
        ListenerDispatch::Delivered
    } else {
        ListenerDispatch::Dropped
    }
}

fn activate_attached_session_on_success(
    sessions: &mut HashSet<acp_nats::AcpSessionId>,
    request_id: &RequestId,
    activate_session_on_success: Option<&acp_nats::AcpSessionId>,
    parsed: Option<&OutgoingHttpMessage>,
) {
    if parsed.and_then(|message| message.id.as_ref()) != Some(request_id) {
        return;
    }

    if parsed.is_some_and(OutgoingHttpMessage::is_success_response)
        && let Some(session_id) = activate_session_on_success
    {
        sessions.insert(session_id.clone());
    }
}

fn pending_response_context(
    message: &IncomingHttpMessage,
    session_id: acp_nats::AcpSessionId,
) -> Option<PendingResponseContext> {
    if message.creates_session() {
        return None;
    }

    let attaches_session = message.attaches_session();
    Some(PendingResponseContext {
        session_id,
        activate_session_on_success: attaches_session,
        inject_session_id: attaches_session,
    })
}

fn targets_unknown_session(
    message: &IncomingHttpMessage,
    session_id: Option<&acp_nats::AcpSessionId>,
    sessions: &HashSet<acp_nats::AcpSessionId>,
) -> bool {
    message.requires_session_id() && session_id.is_some_and(|session_id| !sessions.contains(session_id))
}

fn require_initialized(initialized: bool) -> Result<(), HttpTransportError> {
    if initialized {
        Ok(())
    } else {
        Err(HttpTransportError::bad_request(
            "ACP connection has not been initialized",
        ))
    }
}

fn inject_result_value(raw: &str, field: &str, value: Value) -> Result<String, HttpTransportError> {
    let mut json: Value = serde_json::from_str(raw)
        .map_err(|error| HttpTransportError::internal_with("invalid outbound JSON-RPC payload", error))?;

    let Some(object) = json.as_object_mut() else {
        return Ok(raw.to_string());
    };

    if object.contains_key("error") {
        return Ok(raw.to_string());
    }

    let result = object
        .entry("result")
        .or_insert_with(|| Value::Object(serde_json::Map::new()));

    match result {
        Value::Object(map) => {
            map.entry(field.to_string()).or_insert(value);
        }
        Value::Null => {
            *result = Value::Object(serde_json::Map::from_iter([(field.to_string(), value)]));
        }
        _ => return Ok(raw.to_string()),
    }

    serde_json::to_string(&json)
        .map_err(|error| HttpTransportError::internal_with("invalid outbound JSON-RPC payload", error))
}

fn inject_connection_id_into_initialize_result(
    raw: &str,
    connection_id: &AcpConnectionId,
) -> Result<String, HttpTransportError> {
    inject_result_value(raw, "connectionId", Value::String(connection_id.to_string()))
}

fn inject_session_id_into_response_result(
    raw: &str,
    session_id: &acp_nats::AcpSessionId,
) -> Result<String, HttpTransportError> {
    inject_result_value(raw, "sessionId", Value::String(session_id.as_str().to_string()))
}

fn inject_session_id_into_response_result_or_preserve(
    outbound: String,
    session_id: &acp_nats::AcpSessionId,
    connection_id: &AcpConnectionId,
) -> String {
    match inject_session_id_into_response_result(&outbound, session_id) {
        Ok(updated_outbound) => updated_outbound,
        Err(error) => {
            tracing_error!(%connection_id, error = %error, "Failed to augment outbound session response");
            outbound
        }
    }
}

fn undelivered_response_outcome(
    parsed: Option<&OutgoingHttpMessage>,
    dispatch_outcome: ListenerDispatch,
) -> Option<ListenerDispatch> {
    if !parsed.is_some_and(OutgoingHttpMessage::is_response) {
        return None;
    }

    match dispatch_outcome {
        ListenerDispatch::Missing | ListenerDispatch::Dropped => Some(dispatch_outcome),
        ListenerDispatch::Delivered => None,
    }
}

pub async fn get(State(state): State<AppState>, request: Request) -> Response {
    if let Err(error) = validate_origin(request.headers(), state.bind_host) {
        return error.into_response();
    }

    if is_websocket_request(request.headers()) {
        let (mut parts, _body) = request.into_parts();
        match WebSocketUpgrade::from_request_parts(&mut parts, &state).await {
            Ok(ws) => websocket_response(ws, state),
            Err(_) => HttpTransportError::bad_request("invalid WebSocket upgrade request").into_response(),
        }
    } else {
        match http_get(request.headers().clone(), state).await {
            Ok(response) => response,
            Err(error) => error.into_response(),
        }
    }
}

pub async fn post(headers: HeaderMap, State(state): State<AppState>, body: String) -> Response {
    if let Err(error) = validate_origin(&headers, state.bind_host) {
        return error.into_response();
    }

    match http_post(headers, state, body).await {
        Ok(response) => response,
        Err(error) => error.into_response(),
    }
}

pub async fn delete(headers: HeaderMap, State(state): State<AppState>) -> Response {
    if let Err(error) = validate_origin(&headers, state.bind_host) {
        return error.into_response();
    }

    match http_delete(headers, state).await {
        Ok(response) => response,
        Err(error) => error.into_response(),
    }
}

fn websocket_response(ws: WebSocketUpgrade, state: AppState) -> Response {
    let connection_id = AcpConnectionId::default();
    #[allow(clippy::expect_used)]
    let response_header = HeaderValue::from_str(&connection_id.to_string())
        .expect("generated ACP connection id must be a valid header value");
    let shutdown_rx = state.shutdown_tx.subscribe();
    let mut response = ws.on_upgrade(move |socket| async move {
        if state
            .manager_tx
            .send(ManagerRequest::WebSocket(Box::new(ConnectionRequest {
                connection_id,
                socket,
                shutdown_rx,
            })))
            .is_err()
        {
            tracing_error!("Connection thread is gone; dropping WebSocket");
        }
    });
    response.headers_mut().insert(ACP_CONNECTION_ID_HEADER, response_header);
    response
}

async fn http_post(headers: HeaderMap, state: AppState, body: String) -> Result<Response, HttpTransportError> {
    validate_post_headers(&headers)?;

    let messages = IncomingHttpMessage::parse_all(body)?;
    let mut connection_id = parse_connection_id_header(&headers)?;
    let mut protocol_version = parse_protocol_version_header(&headers)?;
    let session_id = parse_session_id_header(&headers)?;

    let mut json_outcomes = Vec::new();

    for message in messages {
        if !(message.is_request() || message.is_notification() || message.is_response()) {
            return Err(HttpTransportError::bad_request("invalid JSON-RPC message shape"));
        }

        validate_http_context(&message, connection_id.as_ref(), session_id.as_ref())?;

        let (response_tx, response_rx) = oneshot::channel();
        state
            .manager_tx
            .send(ManagerRequest::HttpPost {
                connection_id: connection_id.clone(),
                protocol_version: protocol_version.clone(),
                session_id: session_id.clone(),
                message,
                response: response_tx,
                shutdown_rx: state.shutdown_tx.subscribe(),
            })
            .map_err(|error| HttpTransportError::internal_with("connection manager is unavailable", error))?;

        match response_rx
            .await
            .map_err(|error| HttpTransportError::internal_with("connection manager dropped the request", error))??
        {
            HttpPostOutcome::Accepted => {}
            HttpPostOutcome::Json {
                connection_id: outcome_connection_id,
                protocol_version: outcome_protocol_version,
                body,
            } => {
                connection_id = Some(outcome_connection_id);
                protocol_version = outcome_protocol_version;
                json_outcomes.push(body);
            }
        }
    }

    match json_outcomes.len() {
        0 => Ok(StatusCode::ACCEPTED.into_response()),
        1 => Ok(build_json_response(
            connection_id.unwrap_or_default(),
            protocol_version,
            json_outcomes.remove(0),
        )),
        _ => {
            let values: Vec<Value> = json_outcomes
                .into_iter()
                .map(|body| {
                    serde_json::from_str(&body).map_err(|error| {
                        HttpTransportError::internal_with("invalid outbound JSON-RPC payload", error)
                    })
                })
                .collect::<Result<_, _>>()?;
            let body = serde_json::to_string(&values).map_err(|error| {
                HttpTransportError::internal_with("invalid outbound JSON-RPC payload", error)
            })?;
            Ok(build_json_response(
                connection_id.unwrap_or_default(),
                protocol_version,
                body,
            ))
        }
    }
}

async fn http_get(headers: HeaderMap, state: AppState) -> Result<Response, HttpTransportError> {
    validate_get_headers(&headers)?;

    let connection_id = parse_connection_id_header(&headers)?
        .ok_or(HttpTransportError::bad_request("missing Acp-Connection-Id header"))?;
    let protocol_version = parse_protocol_version_header(&headers)?;

    let (response_tx, response_rx) = oneshot::channel();
    state
        .manager_tx
        .send(ManagerRequest::HttpGet {
            connection_id: connection_id.clone(),
            protocol_version,
            response: response_tx,
        })
        .map_err(|error| HttpTransportError::internal_with("connection manager is unavailable", error))?;

    let outcome = response_rx
        .await
        .map_err(|error| HttpTransportError::internal_with("connection manager dropped the request", error))??;

    let mut response = Sse::new(stream::unfold(outcome.stream, |mut stream| async move {
        stream
            .recv()
            .await
            .map(|item| (Ok::<Event, Infallible>(item.into_event()), stream))
    }))
    .into_response();
    set_transport_headers(
        response.headers_mut(),
        &connection_id,
        outcome.protocol_version.as_ref(),
        None,
    );
    response
        .headers_mut()
        .insert(X_ACCEL_BUFFERING_HEADER, HeaderValue::from_static("no"));
    Ok(response)
}

async fn http_delete(headers: HeaderMap, state: AppState) -> Result<Response, HttpTransportError> {
    let connection_id = parse_connection_id_header(&headers)?
        .ok_or(HttpTransportError::bad_request("missing Acp-Connection-Id header"))?;
    let protocol_version = parse_protocol_version_header(&headers)?;

    let (response_tx, response_rx) = oneshot::channel();
    state
        .manager_tx
        .send(ManagerRequest::HttpDelete {
            connection_id,
            protocol_version,
            response: response_tx,
        })
        .map_err(|error| HttpTransportError::internal_with("connection manager is unavailable", error))?;

    let protocol_version = response_rx
        .await
        .map_err(|error| HttpTransportError::internal_with("connection manager dropped the request", error))??;

    let mut response = StatusCode::ACCEPTED.into_response();
    set_protocol_version_header(response.headers_mut(), protocol_version.as_ref());
    Ok(response)
}

fn validate_post_headers(headers: &HeaderMap) -> Result<(), HttpTransportError> {
    match headers.get(CONTENT_TYPE).and_then(|value| value.to_str().ok()) {
        Some(value) if media_type_matches(value, "application/json") => {}
        _ => {
            return Err(HttpTransportError::unsupported_media_type(
                "Content-Type must be application/json",
            ));
        }
    }

    Ok(())
}

fn validate_get_headers(headers: &HeaderMap) -> Result<(), HttpTransportError> {
    let accept =
        headers
            .get(ACCEPT)
            .and_then(|value| value.to_str().ok())
            .ok_or(HttpTransportError::not_acceptable(
                "Accept must include text/event-stream",
            ))?;

    if !accept_contains(accept, "text/event-stream") {
        return Err(HttpTransportError::not_acceptable(
            "Accept must include text/event-stream",
        ));
    }

    Ok(())
}

fn validate_http_context(
    message: &IncomingHttpMessage,
    connection_id: Option<&AcpConnectionId>,
    session_id: Option<&acp_nats::AcpSessionId>,
) -> Result<(), HttpTransportError> {
    if message.is_initialize() {
        if connection_id.is_some() {
            return Err(HttpTransportError::bad_request(
                "initialize must not include Acp-Connection-Id",
            ));
        }
        if session_id.is_some() {
            return Err(HttpTransportError::bad_request(
                "initialize must not include Acp-Session-Id",
            ));
        }
        return Ok(());
    }

    if connection_id.is_none() {
        return Err(HttpTransportError::bad_request("missing Acp-Connection-Id header"));
    }

    let body_session_id = message.params_session_id()?;
    if message.requires_session_id() && session_id.is_none() {
        return Err(HttpTransportError::bad_request("missing Acp-Session-Id header"));
    }

    if let (Some(header_session_id), Some(body_session_id)) = (session_id, body_session_id.as_ref())
        && header_session_id != body_session_id
    {
        return Err(HttpTransportError::bad_request(
            "Acp-Session-Id header does not match request body sessionId",
        ));
    }

    Ok(())
}

fn accept_contains(header: &str, expected: &str) -> bool {
    header
        .split(',')
        .map(str::trim)
        .any(|media_type| media_type_matches(media_type, expected))
}

fn media_type_matches(header_value: &str, expected: &str) -> bool {
    header_value
        .split(';')
        .next()
        .map(str::trim)
        .is_some_and(|media_type| media_type.eq_ignore_ascii_case(expected))
}

fn validate_origin(headers: &HeaderMap, bind_host: IpAddr) -> Result<(), HttpTransportError> {
    let Some(origin) = headers.get(ORIGIN) else {
        return Ok(());
    };

    let origin = origin
        .to_str()
        .map_err(|error| HttpTransportError::forbidden_with("invalid Origin header", error))?;
    let origin = origin
        .parse::<Uri>()
        .map_err(|error| HttpTransportError::forbidden_with("invalid Origin header", error))?;
    let Some(origin_host) = origin.host() else {
        return Err(HttpTransportError::forbidden("invalid Origin header"));
    };

    let request_host = headers
        .get(HOST)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<Authority>().ok())
        .map(|authority| authority.host().to_owned());

    let allowed = if bind_host.is_loopback() {
        is_loopback_host(origin_host)
    } else if bind_host.is_unspecified() {
        is_loopback_host(origin_host)
            || request_host
                .as_deref()
                .is_some_and(|host| host.eq_ignore_ascii_case(origin_host))
    } else {
        origin_host.eq_ignore_ascii_case(&bind_host.to_string())
            || request_host
                .as_deref()
                .is_some_and(|host| host.eq_ignore_ascii_case(origin_host))
    };

    if allowed {
        Ok(())
    } else {
        Err(HttpTransportError::forbidden("Origin is not allowed"))
    }
}

fn is_loopback_host(host: &str) -> bool {
    matches!(host, "localhost" | "127.0.0.1" | "::1" | "[::1]")
}

fn parse_connection_id_header(headers: &HeaderMap) -> Result<Option<AcpConnectionId>, HttpTransportError> {
    headers
        .get(ACP_CONNECTION_ID_HEADER)
        .map(|value| {
            value
                .to_str()
                .map_err(|error| HttpTransportError::bad_request_with("invalid Acp-Connection-Id header", error))
                .and_then(|value| {
                    AcpConnectionId::parse(value).map_err(|error| {
                        HttpTransportError::bad_request_with("invalid Acp-Connection-Id header", error)
                    })
                })
        })
        .transpose()
}

fn parse_protocol_version_header(headers: &HeaderMap) -> Result<Option<ProtocolVersion>, HttpTransportError> {
    headers
        .get(ACP_PROTOCOL_VERSION_HEADER)
        .map(|value| {
            value
                .to_str()
                .map_err(|error| HttpTransportError::bad_request_with("invalid Acp-Protocol-Version header", error))
                .and_then(|value| {
                    value.parse::<u16>().map(ProtocolVersion::from).map_err(|error| {
                        HttpTransportError::bad_request_with("invalid Acp-Protocol-Version header", error)
                    })
                })
        })
        .transpose()
}

fn parse_session_id_header(headers: &HeaderMap) -> Result<Option<acp_nats::AcpSessionId>, HttpTransportError> {
    headers
        .get(ACP_SESSION_ID_HEADER)
        .map(|value| {
            value
                .to_str()
                .map_err(|error| HttpTransportError::bad_request_with("invalid Acp-Session-Id header", error))
                .and_then(|value| {
                    acp_nats::AcpSessionId::new(value)
                        .map_err(|error| HttpTransportError::bad_request_with("invalid Acp-Session-Id header", error))
                })
        })
        .transpose()
}

fn is_websocket_request(headers: &HeaderMap) -> bool {
    headers
        .get("upgrade")
        .and_then(|value| value.to_str().ok())
        .map(|value| value.eq_ignore_ascii_case("websocket"))
        .unwrap_or(false)
}

fn build_json_response(
    connection_id: AcpConnectionId,
    protocol_version: Option<ProtocolVersion>,
    body: String,
) -> Response {
    let mut response = (StatusCode::OK, body).into_response();
    response
        .headers_mut()
        .insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    set_transport_headers(response.headers_mut(), &connection_id, protocol_version.as_ref(), None);
    response
}

#[allow(clippy::expect_used)]
fn set_transport_headers(
    headers: &mut HeaderMap,
    connection_id: &AcpConnectionId,
    protocol_version: Option<&ProtocolVersion>,
    session_id: Option<&acp_nats::AcpSessionId>,
) {
    headers.insert(
        ACP_CONNECTION_ID_HEADER,
        HeaderValue::from_str(&connection_id.to_string())
            .expect("generated ACP connection id must be a valid header value"),
    );
    set_protocol_version_header(headers, protocol_version);
    if let Some(session_id) = session_id {
        headers.insert(
            ACP_SESSION_ID_HEADER,
            HeaderValue::from_str(session_id.as_str()).expect("generated ACP session id must be a valid header value"),
        );
    }
}

#[allow(clippy::expect_used)]
fn set_protocol_version_header(headers: &mut HeaderMap, protocol_version: Option<&ProtocolVersion>) {
    if let Some(protocol_version) = protocol_version {
        headers.insert(
            ACP_PROTOCOL_VERSION_HEADER,
            HeaderValue::from_str(&protocol_version.to_string())
                .expect("ACP protocol version must be a valid header value"),
        );
    }
}

fn validate_protocol_version_header(
    provided: Option<&ProtocolVersion>,
    negotiated: Option<&ProtocolVersion>,
) -> Result<(), HttpTransportError> {
    if let (Some(provided), Some(negotiated)) = (provided, negotiated)
        && provided != negotiated
    {
        return Err(HttpTransportError::bad_request(
            "Acp-Protocol-Version header does not match initialized protocol version",
        ));
    }

    Ok(())
}

pub async fn run_http_connection<N, J>(
    connection_id: AcpConnectionId,
    nats_client: N,
    js_client: J,
    config: acp_nats::Config,
    mut command_rx: mpsc::UnboundedReceiver<HttpConnectionCommand>,
    mut shutdown_rx: watch::Receiver<bool>,
) where
    N: acp_nats::RequestClient
        + acp_nats::PublishClient
        + acp_nats::FlushClient
        + acp_nats::SubscribeClient
        + Clone
        + 'static,
    J: acp_nats::JetStreamPublisher + acp_nats::JetStreamGetStream + 'static,
    trogon_nats::jetstream::JsMessageOf<J>: trogon_nats::jetstream::JsRequestMessage,
{
    let (agent_write, mut output_read) = tokio::io::duplex(crate::constants::DUPLEX_BUFFER_SIZE);
    let (mut input_write, agent_read) = tokio::io::duplex(crate::constants::DUPLEX_BUFFER_SIZE);

    let incoming = async_compat::Compat::new(agent_read);
    let outgoing = async_compat::Compat::new(agent_write);

    let meter = trogon_telemetry::meter("acp-nats-server");
    let (notification_tx, notification_rx) = tokio::sync::mpsc::channel::<SessionNotification>(64);
    let bridge = Rc::new(Bridge::new(
        nats_client.clone(),
        js_client,
        SystemClock,
        &meter,
        config,
        notification_tx,
    ));

    let (connection, io_task) = AgentSideConnection::new(bridge.clone(), outgoing, incoming, |fut| {
        tokio::task::spawn_local(fut);
    });

    let connection = Rc::new(connection);
    spawn_notification_forwarder(connection.clone(), notification_rx);

    let (input_tx, mut input_rx) = mpsc::channel::<String>(HTTP_CHANNEL_CAPACITY);
    let input_task = tokio::task::spawn_local(async move {
        while let Some(message) = input_rx.recv().await {
            if input_write.write_all(message.as_bytes()).await.is_err() {
                break;
            }
            if input_write.write_all(b"\n").await.is_err() {
                break;
            }
        }
    });

    let (output_tx, mut output_rx) = mpsc::channel::<String>(HTTP_CHANNEL_CAPACITY);
    let output_task = tokio::task::spawn_local(async move {
        let mut reader = tokio::io::BufReader::new(&mut output_read);
        let mut line = String::new();
        loop {
            line.clear();
            match reader.read_line(&mut line).await {
                Ok(0) => break,
                Ok(_) => {
                    let trimmed = line.trim_end_matches(['\r', '\n']);
                    if trimmed.is_empty() {
                        continue;
                    }
                    if output_tx.send(trimmed.to_string()).await.is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });

    let mut client_task =
        tokio::task::spawn_local(client::run(nats_client, connection.clone(), bridge));
    let mut io_task = tokio::task::spawn_local(io_task);

    let mut pending_request: Option<PendingRequest> = None;
    let mut pending_response_sessions = HashMap::<RequestId, PendingResponseContext>::new();
    let mut initialized = false;
    let mut protocol_version: Option<ProtocolVersion> = None;
    let mut sessions = HashSet::<acp_nats::AcpSessionId>::new();
    let mut get_listeners = Vec::<SseSender>::new();

    info!(%connection_id, "HTTP connection established");

    loop {
        tokio::select! {
            command = command_rx.recv() => {
                let Some(command) = command else {
                    break;
                };

                match command {
                    HttpConnectionCommand::Post { protocol_version: header_protocol_version, session_id, message, response } => {
                        if let Err(error) = validate_protocol_version_header(
                            header_protocol_version.as_ref(),
                            protocol_version.as_ref(),
                        ) {
                            let _ = response.send(Err(error));
                            continue;
                        }

                        if !message.is_initialize()
                            && let Err(error) = require_initialized(initialized)
                        {
                            let _ = response.send(Err(error));
                            continue;
                        }

                        if !message.allows_unknown_session()
                            && targets_unknown_session(&message, session_id.as_ref(), &sessions)
                        {
                            let _ = response.send(Err(HttpTransportError::not_found("unknown ACP session")));
                            continue;
                        }

                        if message.is_request() {
                            #[allow(clippy::expect_used)]
                            let request_id = message.id.clone().expect("request must have id");

                            if message.is_initialize() {
                                if initialized || pending_request.is_some() {
                                    let _ = response.send(Err(HttpTransportError::conflict(
                                        "initialize request is already in flight",
                                    )));
                                    continue;
                                }

                                pending_request = Some(PendingRequest::Initialize {
                                    request_id: request_id.clone(),
                                    response,
                                });
                                if input_tx.try_send(message.raw).is_err()
                                    && let Some(PendingRequest::Initialize { response, .. }) =
                                        pending_request.take()
                                {
                                    let _ = response.send(Err(HttpTransportError::internal(
                                        "ACP runtime input queue is full",
                                    )));
                                }
                                continue;
                            }

                            if let Some(session_id) = session_id.clone()
                                && let Some(context) = pending_response_context(&message, session_id)
                            {
                                pending_response_sessions.insert(request_id.clone(), context);
                            }

                            if input_tx.try_send(message.raw).is_err() {
                                pending_response_sessions.remove(&request_id);
                                let _ = response.send(Err(HttpTransportError::internal(
                                    "ACP runtime input queue is full",
                                )));
                                continue;
                            }
                            let _ = response.send(Ok(HttpPostOutcome::Accepted));
                            continue;
                        }

                        if input_tx.try_send(message.raw).is_err() {
                            let _ = response.send(Err(HttpTransportError::internal(
                                "ACP runtime input queue is full",
                            )));
                            continue;
                        }

                        let _ = response.send(Ok(HttpPostOutcome::Accepted));
                    }
                    HttpConnectionCommand::AttachListener {
                        protocol_version: header_protocol_version,
                        response,
                    } => {
                        if let Err(error) = require_initialized(initialized) { let _ = response.send(Err(error)); continue; }

                        if let Err(error) = validate_protocol_version_header(
                            header_protocol_version.as_ref(),
                            protocol_version.as_ref(),
                        ) {
                            let _ = response.send(Err(error));
                            continue;
                        }

                        let (stream_tx, stream_rx) = mpsc::channel(HTTP_CHANNEL_CAPACITY);
                        get_listeners.push(stream_tx);
                        let _ = response.send(Ok(HttpGetOutcome {
                            protocol_version: protocol_version.clone(),
                            stream: stream_rx,
                        }));
                    }
                    HttpConnectionCommand::Close {
                        protocol_version: header_protocol_version,
                        response,
                    } => {
                        if let Err(error) = validate_protocol_version_header(
                            header_protocol_version.as_ref(),
                            protocol_version.as_ref(),
                        ) {
                            let _ = response.send(Err(error));
                            continue;
                        }

                        let _ = response.send(Ok(protocol_version.clone()));
                        break;
                    }
                }
            }
            outbound = output_rx.recv() => {
                let Some(outbound) = outbound else {
                    break;
                };

                let parsed = OutgoingHttpMessage::parse(&outbound);

                if let Some(PendingRequest::Initialize { request_id, .. }) = pending_request.as_ref()
                    && parsed.as_ref().and_then(|message| message.id.as_ref()) == Some(request_id)
                {
                    protocol_version = parsed
                        .as_ref()
                        .and_then(OutgoingHttpMessage::result_protocol_version);
                    initialized = parsed.as_ref().is_some_and(OutgoingHttpMessage::is_success_response)
                        && protocol_version.is_some();

                    let response_body = match inject_connection_id_into_initialize_result(&outbound, &connection_id) {
                        Ok(response_body) => response_body,
                        Err(error) => {
                            if let Some(PendingRequest::Initialize { response, .. }) = pending_request.take() {
                                let _ = response.send(Err(error));
                            }
                            continue;
                        }
                    };

                    if let Some(PendingRequest::Initialize { response, .. }) = pending_request.take() {
                        let _ = response.send(Ok(HttpPostOutcome::Json {
                            connection_id: connection_id.clone(),
                            protocol_version: protocol_version.clone(),
                            body: response_body,
                        }));
                    }
                    continue;
                }

                let mut outbound = outbound;

                if let Some(request_id) = parsed.as_ref().and_then(|message| message.id.as_ref())
                    && let Some(context) = pending_response_sessions.remove(request_id)
                {
                    activate_attached_session_on_success(
                        &mut sessions,
                        request_id,
                        context.activate_session_on_success.then_some(&context.session_id),
                        parsed.as_ref(),
                    );

                    if context.inject_session_id
                        && parsed.as_ref().is_some_and(OutgoingHttpMessage::is_success_response)
                        && parsed.as_ref().and_then(OutgoingHttpMessage::result_session_id).is_none()
                    {
                        outbound = inject_session_id_into_response_result_or_preserve(
                            outbound,
                            &context.session_id,
                            &connection_id,
                        );
                    }
                }

                let frame = SseFrame::json(outbound.clone());
                let parsed = OutgoingHttpMessage::parse(&outbound);

                if let Some(session_id) = parsed.as_ref().and_then(OutgoingHttpMessage::result_session_id) {
                    sessions.insert(session_id);
                }

                // TODO(coverage): no integration test drives the Missing/Dropped warn arms below; add one against the HTTP client outbound loop with zero/full GET listeners.
                if let Some(dispatch_outcome) =
                    undelivered_response_outcome(parsed.as_ref(), dispatch_to_get_listeners(&frame, &mut get_listeners))
                {
                    match dispatch_outcome {
                        ListenerDispatch::Missing => {
                            warn!(%connection_id, "Dropping JSON-RPC response: no active GET listener");
                        }
                        ListenerDispatch::Dropped => {
                            warn!(%connection_id, "Dropping JSON-RPC response: GET listeners closed or stalled");
                        }
                        ListenerDispatch::Delivered => {}
                    }
                }
            }
            result = &mut client_task => {
                match result {
                    Ok(()) => info!(%connection_id, "HTTP client task completed"),
                    Err(error) => warn!(%connection_id, error = %error, "HTTP client task failed"),
                }
                break;
            }
            result = &mut io_task => {
                match result {
                    Ok(Ok(())) => info!(%connection_id, "HTTP IO task completed"),
                    Ok(Err(error)) => warn!(%connection_id, error = %error, "HTTP IO task failed"),
                    Err(error) => warn!(%connection_id, error = %error, "HTTP IO task join failed"),
                }
                break;
            }
            _ = shutdown_rx.wait_for(|&shutdown| shutdown) => {
                info!(%connection_id, "HTTP connection shutting down");
                break;
            }
        }
    }

    fail_pending_initialize_on_close(&mut pending_request);

    input_task.abort();
    output_task.abort();

    if !client_task.is_finished() {
        client_task.abort();
        let _ = client_task.await;
    }
    if !io_task.is_finished() {
        io_task.abort();
        let _ = io_task.await;
    }

    info!(%connection_id, "HTTP connection closed");
}

pub async fn process_manager_request<N, J>(
    request: ManagerRequest,
    http_connections: &mut HashMap<AcpConnectionId, HttpConnectionHandle>,
    websocket_handles: &mut Vec<tokio::task::JoinHandle<()>>,
    http_connection_handles: &mut Vec<tokio::task::JoinHandle<()>>,
    nats_client: &N,
    js_client: &J,
    config: &acp_nats::Config,
) where
    N: acp_nats::RequestClient
        + acp_nats::PublishClient
        + acp_nats::FlushClient
        + acp_nats::SubscribeClient
        + Clone
        + Send
        + 'static,
    J: acp_nats::JetStreamPublisher + acp_nats::JetStreamGetStream + Clone + 'static,
    trogon_nats::jetstream::JsMessageOf<J>: trogon_nats::jetstream::JsRequestMessage,
{
    websocket_handles.retain(|handle| !handle.is_finished());
    http_connection_handles.retain(|handle| !handle.is_finished());
    http_connections.retain(|_, handle| !handle.command_tx.is_closed());

    match request {
        ManagerRequest::WebSocket(request) => {
            let ConnectionRequest {
                connection_id,
                socket,
                shutdown_rx,
            } = *request;
            websocket_handles.push(tokio::task::spawn_local(connection::handle(
                connection_id,
                socket,
                nats_client.clone(),
                js_client.clone(),
                config.clone(),
                shutdown_rx,
            )));
        }
        ManagerRequest::HttpPost {
            connection_id,
            protocol_version,
            session_id,
            message,
            response,
            shutdown_rx,
        } => {
            let connection_id = match connection_id {
                Some(connection_id) => connection_id,
                None => {
                    if !message.is_initialize() {
                        let _ = response.send(Err(HttpTransportError::bad_request("missing Acp-Connection-Id header")));
                        return;
                    }

                    let connection_id = AcpConnectionId::default();
                    let (command_tx, command_rx) = mpsc::unbounded_channel();
                    http_connections.insert(
                        connection_id.clone(),
                        HttpConnectionHandle {
                            command_tx: command_tx.clone(),
                        },
                    );
                    http_connection_handles.push(tokio::task::spawn_local(run_http_connection(
                        connection_id.clone(),
                        nats_client.clone(),
                        js_client.clone(),
                        config.clone(),
                        command_rx,
                        shutdown_rx,
                    )));
                    connection_id
                }
            };

            let Some(handle) = http_connections.get(&connection_id) else {
                let _ = response.send(Err(HttpTransportError::not_found("unknown ACP connection")));
                return;
            };

            if handle
                .command_tx
                .send(HttpConnectionCommand::Post {
                    protocol_version,
                    session_id,
                    message,
                    response,
                })
                .is_err()
            {
                http_connections.remove(&connection_id);
            }
        }
        ManagerRequest::HttpGet {
            connection_id,
            protocol_version,
            response,
        } => {
            let Some(handle) = http_connections.get(&connection_id) else {
                let _ = response.send(Err(HttpTransportError::not_found("unknown ACP connection")));
                return;
            };

            if handle
                .command_tx
                .send(HttpConnectionCommand::AttachListener {
                    protocol_version,
                    response,
                })
                .is_err()
            {
                http_connections.remove(&connection_id);
            }
        }
        ManagerRequest::HttpDelete {
            connection_id,
            protocol_version,
            response,
        } => {
            let Some(handle) = http_connections.remove(&connection_id) else {
                let _ = response.send(Err(HttpTransportError::not_found("unknown ACP connection")));
                return;
            };

            let _ = handle.command_tx.send(HttpConnectionCommand::Close {
                protocol_version,
                response,
            });
        }
    }
}

#[cfg(test)]
mod tests;
