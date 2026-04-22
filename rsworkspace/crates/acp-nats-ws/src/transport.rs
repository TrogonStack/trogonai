use crate::acp_connection_id::AcpConnectionId;
use crate::connection;
use crate::constants::{
    ACP_CONNECTION_ID_HEADER, ACP_PROTOCOL_VERSION_HEADER, ACP_SESSION_ID_HEADER,
    X_ACCEL_BUFFERING_HEADER,
};
use acp_nats::{StdJsonSerialize, agent::Bridge, client, spawn_notification_forwarder};
use agent_client_protocol::{AgentSideConnection, ProtocolVersion, RequestId, SessionNotification};
use axum::extract::FromRequestParts;
use axum::extract::Request;
use axum::extract::State;
use axum::extract::ws::{WebSocket, WebSocketUpgrade};
use axum::http::header::{ACCEPT, CONTENT_TYPE, HOST, ORIGIN};
use axum::http::{HeaderMap, HeaderValue, StatusCode, Uri, uri::Authority};
use axum::response::sse::{Event, Sse};
use axum::response::{IntoResponse, Response};
use futures_util::{StreamExt, stream};
use serde::Deserialize;
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::convert::Infallible;
use std::net::IpAddr;
use std::rc::Rc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
use tokio::sync::{mpsc, oneshot, watch};
use tracing::{error, info, warn};
use trogon_std::time::SystemClock;
use uuid::Uuid;

const HTTP_CHANNEL_CAPACITY: usize = 64;

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
        session_id: acp_nats::AcpSessionId,
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
    Live {
        connection_id: AcpConnectionId,
        protocol_version: Option<ProtocolVersion>,
        session_id: Option<acp_nats::AcpSessionId>,
        stream: SseReceiver,
    },
    Buffered {
        connection_id: AcpConnectionId,
        protocol_version: Option<ProtocolVersion>,
        session_id: Option<acp_nats::AcpSessionId>,
        events: Vec<SseFrame>,
    },
}

#[derive(Debug)]
pub struct HttpGetOutcome {
    pub protocol_version: Option<ProtocolVersion>,
    pub stream: SseReceiver,
}

#[derive(Debug)]
pub enum HttpTransportError {
    BadRequest {
        message: &'static str,
        source: Option<BoxError>,
    },
    NotFound {
        message: &'static str,
        source: Option<BoxError>,
    },
    Conflict {
        message: &'static str,
        source: Option<BoxError>,
    },
    Forbidden {
        message: &'static str,
        source: Option<BoxError>,
    },
    UnsupportedMediaType {
        message: &'static str,
        source: Option<BoxError>,
    },
    NotAcceptable {
        message: &'static str,
        source: Option<BoxError>,
    },
    NotImplemented {
        message: &'static str,
        source: Option<BoxError>,
    },
    Internal {
        message: &'static str,
        source: Option<BoxError>,
    },
}

impl std::fmt::Display for HttpTransportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.message())
    }
}

impl std::error::Error for HttpTransportError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.source_ref()
    }
}

impl HttpTransportError {
    fn message(&self) -> &'static str {
        match self {
            Self::BadRequest { message, .. }
            | Self::NotFound { message, .. }
            | Self::Conflict { message, .. }
            | Self::Forbidden { message, .. }
            | Self::UnsupportedMediaType { message, .. }
            | Self::NotAcceptable { message, .. }
            | Self::NotImplemented { message, .. }
            | Self::Internal { message, .. } => message,
        }
    }

    fn source_ref(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::BadRequest { source, .. }
            | Self::NotFound { source, .. }
            | Self::Conflict { source, .. }
            | Self::Forbidden { source, .. }
            | Self::UnsupportedMediaType { source, .. }
            | Self::NotAcceptable { source, .. }
            | Self::NotImplemented { source, .. }
            | Self::Internal { source, .. } => source
                .as_deref()
                .map(|source| source as &(dyn std::error::Error + 'static)),
        }
    }

    fn bad_request(message: &'static str) -> Self {
        Self::BadRequest {
            message,
            source: None,
        }
    }

    fn bad_request_with(
        message: &'static str,
        source: impl std::error::Error + Send + 'static,
    ) -> Self {
        Self::BadRequest {
            message,
            source: Some(Box::new(source)),
        }
    }

    fn not_found(message: &'static str) -> Self {
        Self::NotFound {
            message,
            source: None,
        }
    }

    fn conflict(message: &'static str) -> Self {
        Self::Conflict {
            message,
            source: None,
        }
    }

    fn forbidden(message: &'static str) -> Self {
        Self::Forbidden {
            message,
            source: None,
        }
    }

    fn forbidden_with(
        message: &'static str,
        source: impl std::error::Error + Send + 'static,
    ) -> Self {
        Self::Forbidden {
            message,
            source: Some(Box::new(source)),
        }
    }

    fn unsupported_media_type(message: &'static str) -> Self {
        Self::UnsupportedMediaType {
            message,
            source: None,
        }
    }

    fn not_acceptable(message: &'static str) -> Self {
        Self::NotAcceptable {
            message,
            source: None,
        }
    }

    fn not_implemented(message: &'static str) -> Self {
        Self::NotImplemented {
            message,
            source: None,
        }
    }

    fn internal(message: &'static str) -> Self {
        Self::Internal {
            message,
            source: None,
        }
    }

    fn internal_with(
        message: &'static str,
        source: impl std::error::Error + Send + 'static,
    ) -> Self {
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
        session_id: acp_nats::AcpSessionId,
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
    Empty {
        event_id: String,
    },
    Json {
        event_id: Option<String>,
        json: String,
    },
}

impl SseFrame {
    fn prime() -> Self {
        Self::Empty {
            event_id: Uuid::new_v4().to_string(),
        }
    }

    fn json(json: String) -> Self {
        Self::Json {
            event_id: Some(Uuid::new_v4().to_string()),
            json,
        }
    }

    fn into_event(self) -> Event {
        match self {
            Self::Empty { event_id } => Event::default().id(event_id).data(""),
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
    Live {
        request_id: RequestId,
        capture_protocol_version: bool,
        session_id: Option<acp_nats::AcpSessionId>,
        sender: SseSender,
    },
    Buffered {
        request_id: RequestId,
        events: Vec<SseFrame>,
        response: oneshot::Sender<Result<HttpPostOutcome, HttpTransportError>>,
    },
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
        let trimmed = raw.trim_start();
        if trimmed.starts_with('[') {
            return Err(HttpTransportError::not_implemented(
                "batch JSON-RPC requests are not supported",
            ));
        }

        let value = serde_json::from_str::<Value>(&raw).map_err(|error| {
            HttpTransportError::bad_request_with("invalid JSON-RPC payload", error)
        })?;
        let (has_result, has_error) = value
            .as_object()
            .map(|object| (object.contains_key("result"), object.contains_key("error")))
            .unwrap_or((false, false));
        let mut parsed = serde_json::from_value::<Self>(value).map_err(|error| {
            HttpTransportError::bad_request_with("invalid JSON-RPC payload", error)
        })?;
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

    fn is_session_new(&self) -> bool {
        self.method_name() == Some("session/new")
    }

    fn requires_session_id(&self) -> bool {
        if self.is_response() {
            return true;
        }

        match self.method_name() {
            Some("initialize") | Some("authenticate") | Some("session/new")
            | Some("session/list") => false,
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
            .map_err(|error| {
                HttpTransportError::bad_request_with("invalid sessionId in request body", error)
            })
    }
}

#[derive(Debug, Deserialize)]
struct OutgoingHttpMessage {
    id: Option<RequestId>,
    params: Option<Value>,
    result: Option<Value>,
}

impl OutgoingHttpMessage {
    fn parse(raw: &str) -> Option<Self> {
        serde_json::from_str(raw).ok()
    }

    fn params_session_id(&self) -> Option<acp_nats::AcpSessionId> {
        let params = self.params.as_ref()?;
        let session_id = params.get("sessionId")?.as_str()?;
        acp_nats::AcpSessionId::new(session_id).ok()
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

enum LiveFrameOutcome {
    Keep,
    Clear,
    Drop,
}

enum BufferedFrameOutcome {
    Buffered,
    Routed,
    Finalize {
        session_id: Option<acp_nats::AcpSessionId>,
    },
}

enum ListenerDispatch {
    Missing,
    Delivered,
    Dropped,
}

fn try_send_sse_frame(sender: &SseSender, frame: &SseFrame) -> bool {
    match sender.try_send(frame.clone()) {
        Ok(()) => true,
        Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => false,
        Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => false,
    }
}

fn dispatch_to_get_listeners(
    frame: &SseFrame,
    session_id: &acp_nats::AcpSessionId,
    get_listeners: &mut HashMap<acp_nats::AcpSessionId, Vec<SseSender>>,
) -> ListenerDispatch {
    let Some(_) = get_listeners.get(session_id) else {
        return ListenerDispatch::Missing;
    };

    let (outcome, remove_session) = {
        let listeners = get_listeners
            .get_mut(session_id)
            .expect("session listeners must exist after presence check");
        let mut delivered = false;
        let mut retained = Vec::with_capacity(listeners.len());

        for listener in listeners.drain(..) {
            if delivered {
                retained.push(listener);
                continue;
            }

            match listener.try_send(frame.clone()) {
                Ok(()) => {
                    delivered = true;
                    retained.push(listener);
                }
                Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {
                    warn!(session_id = %session_id, "Dropping stalled HTTP SSE listener");
                }
                Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {}
            }
        }
        *listeners = retained;

        (
            if delivered {
                ListenerDispatch::Delivered
            } else {
                ListenerDispatch::Dropped
            },
            listeners.is_empty(),
        )
    };

    if remove_session {
        get_listeners.remove(session_id);
    }

    outcome
}

fn route_live_frame(
    frame: &SseFrame,
    parsed: Option<&OutgoingHttpMessage>,
    request_id: &RequestId,
    request_session_id: Option<&acp_nats::AcpSessionId>,
    sender: &SseSender,
    get_listeners: &mut HashMap<acp_nats::AcpSessionId, Vec<SseSender>>,
) -> LiveFrameOutcome {
    if parsed.and_then(|message| message.id.as_ref()) == Some(request_id) {
        return if try_send_sse_frame(sender, frame) {
            LiveFrameOutcome::Clear
        } else {
            warn!(request_id = ?request_id, "Dropping stalled HTTP POST response stream");
            LiveFrameOutcome::Drop
        };
    }

    if let Some(frame_session_id) = parsed.and_then(OutgoingHttpMessage::params_session_id) {
        if request_session_id == Some(&frame_session_id) {
            return if try_send_sse_frame(sender, frame) {
                LiveFrameOutcome::Keep
            } else {
                warn!(request_id = ?request_id, session_id = %frame_session_id, "Dropping stalled HTTP POST response stream");
                LiveFrameOutcome::Drop
            };
        }

        match dispatch_to_get_listeners(frame, &frame_session_id, get_listeners) {
            ListenerDispatch::Delivered | ListenerDispatch::Dropped => {
                return LiveFrameOutcome::Keep;
            }
            ListenerDispatch::Missing => {}
        }
    }

    if try_send_sse_frame(sender, frame) {
        LiveFrameOutcome::Keep
    } else {
        warn!(request_id = ?request_id, "Dropping stalled HTTP POST response stream");
        LiveFrameOutcome::Drop
    }
}

fn route_buffered_frame(
    frame: &SseFrame,
    parsed: Option<&OutgoingHttpMessage>,
    request_id: &RequestId,
    events: &mut Vec<SseFrame>,
    get_listeners: &mut HashMap<acp_nats::AcpSessionId, Vec<SseSender>>,
) -> BufferedFrameOutcome {
    if parsed.and_then(|message| message.id.as_ref()) == Some(request_id) {
        events.push(frame.clone());
        return BufferedFrameOutcome::Finalize {
            session_id: parsed.and_then(OutgoingHttpMessage::result_session_id),
        };
    }

    if let Some(frame_session_id) = parsed.and_then(OutgoingHttpMessage::params_session_id) {
        match dispatch_to_get_listeners(frame, &frame_session_id, get_listeners) {
            ListenerDispatch::Delivered | ListenerDispatch::Dropped => {
                return BufferedFrameOutcome::Routed;
            }
            ListenerDispatch::Missing => {}
        }
    }

    events.push(frame.clone());
    BufferedFrameOutcome::Buffered
}

pub async fn get(State(state): State<AppState>, request: Request) -> Response {
    if let Err(error) = validate_origin(request.headers(), state.bind_host) {
        return error.into_response();
    }

    if is_websocket_request(request.headers()) {
        let (mut parts, _body) = request.into_parts();
        match WebSocketUpgrade::from_request_parts(&mut parts, &state).await {
            Ok(ws) => websocket_response(ws, state),
            Err(_) => {
                HttpTransportError::bad_request("invalid WebSocket upgrade request").into_response()
            }
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
    let connection_id = AcpConnectionId::new();
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
            error!("Connection thread is gone; dropping WebSocket");
        }
    });
    response
        .headers_mut()
        .insert(ACP_CONNECTION_ID_HEADER, response_header);
    response
}

async fn http_post(
    headers: HeaderMap,
    state: AppState,
    body: String,
) -> Result<Response, HttpTransportError> {
    validate_post_headers(&headers)?;

    let message = IncomingHttpMessage::parse(body)?;
    if !(message.is_request() || message.is_notification() || message.is_response()) {
        return Err(HttpTransportError::bad_request(
            "invalid JSON-RPC message shape",
        ));
    }

    let connection_id = parse_connection_id_header(&headers)?;
    let protocol_version = parse_protocol_version_header(&headers)?;
    let session_id = parse_session_id_header(&headers)?;

    validate_http_context(&message, connection_id.as_ref(), session_id.as_ref())?;

    let (response_tx, response_rx) = oneshot::channel();
    state
        .manager_tx
        .send(ManagerRequest::HttpPost {
            connection_id,
            protocol_version,
            session_id,
            message,
            response: response_tx,
            shutdown_rx: state.shutdown_tx.subscribe(),
        })
        .map_err(|error| {
            HttpTransportError::internal_with("connection manager is unavailable", error)
        })?;

    match response_rx.await.map_err(|error| {
        HttpTransportError::internal_with("connection manager dropped the request", error)
    })?? {
        HttpPostOutcome::Accepted => Ok(StatusCode::ACCEPTED.into_response()),
        HttpPostOutcome::Live {
            connection_id,
            protocol_version,
            session_id,
            stream,
        } => Ok(build_sse_response(
            connection_id,
            protocol_version,
            session_id,
            stream,
        )),
        HttpPostOutcome::Buffered {
            connection_id,
            protocol_version,
            session_id,
            events,
        } => Ok(build_buffered_sse_response(
            connection_id,
            protocol_version,
            session_id,
            events,
        )),
    }
}

async fn http_get(headers: HeaderMap, state: AppState) -> Result<Response, HttpTransportError> {
    validate_get_headers(&headers)?;

    let connection_id = parse_connection_id_header(&headers)?.ok_or(
        HttpTransportError::bad_request("missing Acp-Connection-Id header"),
    )?;
    let protocol_version = parse_protocol_version_header(&headers)?;
    let session_id = parse_session_id_header(&headers)?.ok_or(HttpTransportError::bad_request(
        "missing Acp-Session-Id header",
    ))?;

    let (response_tx, response_rx) = oneshot::channel();
    state
        .manager_tx
        .send(ManagerRequest::HttpGet {
            connection_id: connection_id.clone(),
            session_id: session_id.clone(),
            protocol_version,
            response: response_tx,
        })
        .map_err(|error| {
            HttpTransportError::internal_with("connection manager is unavailable", error)
        })?;

    let outcome = response_rx.await.map_err(|error| {
        HttpTransportError::internal_with("connection manager dropped the request", error)
    })??;

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
        Some(&session_id),
    );
    response
        .headers_mut()
        .insert(X_ACCEL_BUFFERING_HEADER, HeaderValue::from_static("no"));
    Ok(response)
}

async fn http_delete(headers: HeaderMap, state: AppState) -> Result<Response, HttpTransportError> {
    let connection_id = parse_connection_id_header(&headers)?.ok_or(
        HttpTransportError::bad_request("missing Acp-Connection-Id header"),
    )?;
    let protocol_version = parse_protocol_version_header(&headers)?;

    let (response_tx, response_rx) = oneshot::channel();
    state
        .manager_tx
        .send(ManagerRequest::HttpDelete {
            connection_id,
            protocol_version,
            response: response_tx,
        })
        .map_err(|error| {
            HttpTransportError::internal_with("connection manager is unavailable", error)
        })?;

    let protocol_version = response_rx.await.map_err(|error| {
        HttpTransportError::internal_with("connection manager dropped the request", error)
    })??;

    let mut response = StatusCode::ACCEPTED.into_response();
    set_protocol_version_header(response.headers_mut(), protocol_version.as_ref());
    Ok(response)
}

fn validate_post_headers(headers: &HeaderMap) -> Result<(), HttpTransportError> {
    match headers
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
    {
        Some(value) if media_type_matches(value, "application/json") => {}
        _ => {
            return Err(HttpTransportError::unsupported_media_type(
                "Content-Type must be application/json",
            ));
        }
    }

    let accept = headers
        .get(ACCEPT)
        .and_then(|value| value.to_str().ok())
        .ok_or(HttpTransportError::not_acceptable(
            "Accept must include application/json and text/event-stream",
        ))?;

    if !accept_contains(accept, "application/json") || !accept_contains(accept, "text/event-stream")
    {
        return Err(HttpTransportError::not_acceptable(
            "Accept must include application/json and text/event-stream",
        ));
    }

    Ok(())
}

fn validate_get_headers(headers: &HeaderMap) -> Result<(), HttpTransportError> {
    let accept = headers
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
        return Err(HttpTransportError::bad_request(
            "missing Acp-Connection-Id header",
        ));
    }

    let body_session_id = message.params_session_id()?;
    if message.requires_session_id() && session_id.is_none() {
        return Err(HttpTransportError::bad_request(
            "missing Acp-Session-Id header",
        ));
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

fn parse_connection_id_header(
    headers: &HeaderMap,
) -> Result<Option<AcpConnectionId>, HttpTransportError> {
    headers
        .get(ACP_CONNECTION_ID_HEADER)
        .map(|value| {
            value
                .to_str()
                .map_err(|error| {
                    HttpTransportError::bad_request_with("invalid Acp-Connection-Id header", error)
                })
                .and_then(|value| {
                    AcpConnectionId::parse(value).map_err(|error| {
                        HttpTransportError::bad_request_with(
                            "invalid Acp-Connection-Id header",
                            error,
                        )
                    })
                })
        })
        .transpose()
}

fn parse_protocol_version_header(
    headers: &HeaderMap,
) -> Result<Option<ProtocolVersion>, HttpTransportError> {
    headers
        .get(ACP_PROTOCOL_VERSION_HEADER)
        .map(|value| {
            value
                .to_str()
                .map_err(|error| {
                    HttpTransportError::bad_request_with(
                        "invalid Acp-Protocol-Version header",
                        error,
                    )
                })
                .and_then(|value| {
                    value
                        .parse::<u16>()
                        .map(ProtocolVersion::from)
                        .map_err(|error| {
                            HttpTransportError::bad_request_with(
                                "invalid Acp-Protocol-Version header",
                                error,
                            )
                        })
                })
        })
        .transpose()
}

fn parse_session_id_header(
    headers: &HeaderMap,
) -> Result<Option<acp_nats::AcpSessionId>, HttpTransportError> {
    headers
        .get(ACP_SESSION_ID_HEADER)
        .map(|value| {
            value
                .to_str()
                .map_err(|error| {
                    HttpTransportError::bad_request_with("invalid Acp-Session-Id header", error)
                })
                .and_then(|value| {
                    acp_nats::AcpSessionId::new(value).map_err(|error| {
                        HttpTransportError::bad_request_with("invalid Acp-Session-Id header", error)
                    })
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

fn build_sse_response(
    connection_id: AcpConnectionId,
    protocol_version: Option<ProtocolVersion>,
    session_id: Option<acp_nats::AcpSessionId>,
    stream: SseReceiver,
) -> Response {
    let primed_stream =
        stream::once(async { Ok::<Event, Infallible>(SseFrame::prime().into_event()) }).chain(
            stream::unfold(stream, |mut stream| async move {
                stream
                    .recv()
                    .await
                    .map(|item| (Ok::<Event, Infallible>(item.into_event()), stream))
            }),
        );
    let mut response = Sse::new(primed_stream).into_response();
    set_transport_headers(
        response.headers_mut(),
        &connection_id,
        protocol_version.as_ref(),
        session_id.as_ref(),
    );
    response
        .headers_mut()
        .insert(X_ACCEL_BUFFERING_HEADER, HeaderValue::from_static("no"));
    response
}

fn build_buffered_sse_response(
    connection_id: AcpConnectionId,
    protocol_version: Option<ProtocolVersion>,
    session_id: Option<acp_nats::AcpSessionId>,
    events: Vec<SseFrame>,
) -> Response {
    let stream = stream::iter(
        std::iter::once(SseFrame::prime())
            .chain(events)
            .map(|item| Ok::<Event, Infallible>(item.into_event())),
    );
    let mut response = Sse::new(stream).into_response();
    set_transport_headers(
        response.headers_mut(),
        &connection_id,
        protocol_version.as_ref(),
        session_id.as_ref(),
    );
    response
        .headers_mut()
        .insert(X_ACCEL_BUFFERING_HEADER, HeaderValue::from_static("no"));
    response
}

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
            HeaderValue::from_str(session_id.as_str())
                .expect("generated ACP session id must be a valid header value"),
        );
    }
}

fn set_protocol_version_header(
    headers: &mut HeaderMap,
    protocol_version: Option<&ProtocolVersion>,
) {
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

    let meter = acp_telemetry::meter("acp-nats-ws");
    let (notification_tx, notification_rx) = tokio::sync::mpsc::channel::<SessionNotification>(64);
    let bridge = Rc::new(Bridge::new(
        nats_client.clone(),
        js_client,
        SystemClock,
        &meter,
        config,
        notification_tx,
    ));

    let (connection, io_task) =
        AgentSideConnection::new(bridge.clone(), outgoing, incoming, |fut| {
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

    let mut client_task = tokio::task::spawn_local(client::run(
        nats_client,
        connection.clone(),
        bridge,
        StdJsonSerialize,
    ));
    let mut io_task = tokio::task::spawn_local(io_task);

    let mut pending_request: Option<PendingRequest> = None;
    let mut protocol_version: Option<ProtocolVersion> = None;
    let mut sessions = HashSet::<acp_nats::AcpSessionId>::new();
    let mut get_listeners = HashMap::<acp_nats::AcpSessionId, Vec<SseSender>>::new();

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

                        if message.is_request() {
                            if pending_request.is_some() {
                                let _ = response.send(Err(HttpTransportError::conflict(
                                    "only one in-flight HTTP request is supported per ACP connection",
                                )));
                                continue;
                            }

                            if message.is_session_new() {
                                pending_request = Some(PendingRequest::Buffered {
                                    request_id: message.id.clone().expect("request must have id"),
                                    events: Vec::new(),
                                    response,
                                });
                                if input_tx.try_send(message.raw).is_err()
                                    && let Some(PendingRequest::Buffered { response, .. }) =
                                        pending_request.take()
                                {
                                    let _ = response.send(Err(HttpTransportError::internal(
                                        "ACP runtime input queue is full",
                                    )));
                                }
                                continue;
                            }

                            if let Some(session_id) = session_id.clone() {
                                sessions.insert(session_id);
                            }

                            let (stream_tx, stream_rx) = mpsc::channel(HTTP_CHANNEL_CAPACITY);
                            pending_request = Some(PendingRequest::Live {
                                request_id: message.id.clone().expect("request must have id"),
                                capture_protocol_version: message.is_initialize(),
                                session_id: session_id.clone(),
                                sender: stream_tx,
                            });
                            if input_tx.try_send(message.raw).is_err() {
                                pending_request = None;
                                let _ = response.send(Err(HttpTransportError::internal(
                                    "ACP runtime input queue is full",
                                )));
                                continue;
                            }
                            let _ = response.send(Ok(HttpPostOutcome::Live {
                                connection_id: connection_id.clone(),
                                protocol_version: protocol_version.clone(),
                                session_id,
                                stream: stream_rx,
                            }));
                            continue;
                        }

                        if let Some(session_id) = session_id.clone() {
                            sessions.insert(session_id);
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
                        session_id,
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

                        if !sessions.contains(&session_id) {
                            let _ = response
                                .send(Err(HttpTransportError::not_found("unknown ACP session")));
                            continue;
                        }

                        let (stream_tx, stream_rx) = mpsc::channel(HTTP_CHANNEL_CAPACITY);
                        get_listeners.entry(session_id).or_default().push(stream_tx);
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

                let frame = SseFrame::json(outbound.clone());
                let parsed = OutgoingHttpMessage::parse(&outbound);

                if let Some(pending) = pending_request.as_mut() {
                    match pending {
                        PendingRequest::Live {
                            request_id,
                            capture_protocol_version,
                            session_id,
                            sender,
                        } => {
                            if *capture_protocol_version
                                && parsed.as_ref().and_then(|message| message.id.as_ref())
                                    == Some(request_id)
                            {
                                protocol_version = parsed
                                    .as_ref()
                                    .and_then(OutgoingHttpMessage::result_protocol_version);
                            }

                            match route_live_frame(
                                &frame,
                                parsed.as_ref(),
                                request_id,
                                session_id.as_ref(),
                                sender,
                                &mut get_listeners,
                            ) {
                                LiveFrameOutcome::Keep => {}
                                LiveFrameOutcome::Clear | LiveFrameOutcome::Drop => {
                                    pending_request = None;
                                }
                            }
                            continue;
                        }
                        PendingRequest::Buffered { request_id, events, .. } => {
                            match route_buffered_frame(
                                &frame,
                                parsed.as_ref(),
                                request_id,
                                events,
                                &mut get_listeners,
                            ) {
                                BufferedFrameOutcome::Buffered | BufferedFrameOutcome::Routed => {}
                                BufferedFrameOutcome::Finalize { session_id } => {
                                    if let Some(session_id) = session_id.clone() {
                                        sessions.insert(session_id);
                                    }
                                    let events = std::mem::take(events);
                                    if let Some(PendingRequest::Buffered { response, .. }) =
                                        pending_request.take()
                                    {
                                        let _ = response.send(Ok(HttpPostOutcome::Buffered {
                                            connection_id: connection_id.clone(),
                                            protocol_version: protocol_version.clone(),
                                            session_id,
                                            events,
                                        }));
                                    }
                                }
                            }
                            continue;
                        }
                    }
                }

                let Some(session_id) = parsed.and_then(|message| message.params_session_id()) else {
                    continue;
                };

                let _ = dispatch_to_get_listeners(&frame, &session_id, &mut get_listeners);
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

    if let Some(PendingRequest::Buffered { response, .. }) = pending_request.take() {
        let _ = response.send(Err(HttpTransportError::internal(
            "HTTP connection closed before the request completed",
        )));
    }

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
                        let _ = response.send(Err(HttpTransportError::bad_request(
                            "missing Acp-Connection-Id header",
                        )));
                        return;
                    }

                    let connection_id = AcpConnectionId::new();
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
            session_id,
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
                    session_id,
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
mod tests {
    use super::*;
    use acp_nats::Config;
    use axum::body::{Body, to_bytes};
    use axum::http::Request as HttpRequest;
    use axum::http::header::{HOST, ORIGIN};
    use serde_json::{Value, json};
    use std::net::{IpAddr, Ipv4Addr};
    use tokio::sync::{mpsc, oneshot, watch};
    use trogon_nats::AdvancedMockNatsClient;

    #[derive(Clone)]
    struct MockJs {
        publisher: trogon_nats::jetstream::MockJetStreamPublisher,
        consumer_factory: trogon_nats::jetstream::MockJetStreamConsumerFactory,
    }

    impl MockJs {
        fn new() -> Self {
            Self {
                publisher: trogon_nats::jetstream::MockJetStreamPublisher::new(),
                consumer_factory: trogon_nats::jetstream::MockJetStreamConsumerFactory::new(),
            }
        }
    }

    impl trogon_nats::jetstream::JetStreamPublisher for MockJs {
        type PublishError = trogon_nats::mocks::MockError;
        type AckFuture = std::future::Ready<
            Result<async_nats::jetstream::publish::PublishAck, Self::PublishError>,
        >;

        async fn publish_with_headers<S: async_nats::subject::ToSubject + Send>(
            &self,
            subject: S,
            headers: async_nats::HeaderMap,
            payload: bytes::Bytes,
        ) -> Result<Self::AckFuture, Self::PublishError> {
            self.publisher
                .publish_with_headers(subject, headers, payload)
                .await
        }
    }

    impl trogon_nats::jetstream::JetStreamGetStream for MockJs {
        type Error = trogon_nats::mocks::MockError;
        type Stream = trogon_nats::jetstream::MockJetStreamStream;

        async fn get_stream<T: AsRef<str> + Send>(
            &self,
            stream_name: T,
        ) -> Result<Self::Stream, Self::Error> {
            self.consumer_factory.get_stream(stream_name).await
        }
    }

    fn test_config() -> Config {
        Config::new(
            acp_nats::AcpPrefix::new("acp").unwrap(),
            acp_nats::NatsConfig {
                servers: vec!["localhost:4222".to_string()],
                auth: trogon_nats::NatsAuth::None,
            },
        )
    }

    fn test_state() -> (AppState, mpsc::UnboundedReceiver<ManagerRequest>) {
        let (manager_tx, manager_rx) = mpsc::unbounded_channel();
        let (shutdown_tx, _) = watch::channel(false);
        (
            AppState {
                bind_host: IpAddr::V4(Ipv4Addr::LOCALHOST),
                manager_tx,
                shutdown_tx,
            },
            manager_rx,
        )
    }

    async fn json_event_body(response: Response) -> Vec<Value> {
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let body = String::from_utf8(bytes.to_vec()).unwrap();
        body.lines()
            .filter_map(|line| line.strip_prefix("data: "))
            .filter(|json| !json.is_empty())
            .map(|json| serde_json::from_str(json).unwrap())
            .collect()
    }

    fn frame_json(frame: &SseFrame) -> &str {
        match frame {
            SseFrame::Json { json, .. } => json,
            SseFrame::Empty { .. } => panic!("expected JSON SSE frame"),
        }
    }

    fn post_headers() -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        headers.insert(
            ACCEPT,
            HeaderValue::from_static("application/json, text/event-stream"),
        );
        headers
    }

    fn get_headers() -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(ACCEPT, HeaderValue::from_static("text/event-stream"));
        headers
    }

    fn session_id() -> acp_nats::AcpSessionId {
        acp_nats::AcpSessionId::new("session-1").unwrap()
    }

    #[test]
    fn http_transport_error_into_response_maps_status_codes() {
        let cases = [
            (
                HttpTransportError::bad_request("bad"),
                StatusCode::BAD_REQUEST,
            ),
            (
                HttpTransportError::not_found("missing"),
                StatusCode::NOT_FOUND,
            ),
            (
                HttpTransportError::conflict("conflict"),
                StatusCode::CONFLICT,
            ),
            (
                HttpTransportError::forbidden("forbidden"),
                StatusCode::FORBIDDEN,
            ),
            (
                HttpTransportError::unsupported_media_type("unsupported"),
                StatusCode::UNSUPPORTED_MEDIA_TYPE,
            ),
            (
                HttpTransportError::not_acceptable("not-acceptable"),
                StatusCode::NOT_ACCEPTABLE,
            ),
            (
                HttpTransportError::not_implemented("not-implemented"),
                StatusCode::NOT_IMPLEMENTED,
            ),
            (
                HttpTransportError::internal("internal"),
                StatusCode::INTERNAL_SERVER_ERROR,
            ),
        ];

        for (error, status) in cases {
            assert_eq!(error.into_response().status(), status);
        }

        let sourced = IncomingHttpMessage::parse("{".to_string()).unwrap_err();
        assert_eq!(sourced.into_response().status(), StatusCode::BAD_REQUEST);
    }

    #[test]
    fn http_transport_error_display_and_source_chain_return_expected_values() {
        use std::error::Error as _;

        assert_eq!(HttpTransportError::bad_request("bad").to_string(), "bad");
        assert!(HttpTransportError::bad_request("bad").source().is_none());
        assert!(HttpTransportError::not_found("missing").source().is_none());
        assert_eq!(
            HttpTransportError::conflict("conflict").to_string(),
            "conflict"
        );
        assert!(HttpTransportError::conflict("conflict").source().is_none());
        assert_eq!(
            HttpTransportError::forbidden("forbidden").to_string(),
            "forbidden"
        );
        assert!(
            HttpTransportError::forbidden("forbidden")
                .source()
                .is_none()
        );
        assert!(
            HttpTransportError::unsupported_media_type("unsupported")
                .source()
                .is_none()
        );
        assert!(
            HttpTransportError::not_acceptable("not-acceptable")
                .source()
                .is_none()
        );
        assert!(
            HttpTransportError::not_implemented("not-implemented")
                .source()
                .is_none()
        );
        assert_eq!(
            HttpTransportError::internal("internal").to_string(),
            "internal"
        );
        assert!(HttpTransportError::internal("internal").source().is_none());

        let invalid_json = IncomingHttpMessage::parse("{".to_string()).unwrap_err();
        assert_eq!(invalid_json.to_string(), "invalid JSON-RPC payload");
        assert!(invalid_json.source().is_some());
    }

    #[test]
    fn incoming_http_message_parses_and_classifies_shapes() {
        let request = IncomingHttpMessage::parse(
            r#"{"jsonrpc":"2.0","id":1,"method":"session/new","params":{"cwd":".","mcpServers":[]}}"#
                .to_string(),
        )
        .unwrap();
        assert!(request.is_request());
        assert!(request.is_session_new());

        let notification =
            IncomingHttpMessage::parse(r#"{"jsonrpc":"2.0","method":"initialized"}"#.to_string())
                .unwrap();
        assert!(notification.is_notification());
        assert!(!notification.requires_session_id());

        let response = IncomingHttpMessage::parse(
            r#"{"jsonrpc":"2.0","id":99,"result":{"ok":true}}"#.to_string(),
        )
        .unwrap();
        assert!(response.is_response());
        assert!(response.requires_session_id());

        let null_response =
            IncomingHttpMessage::parse(r#"{"jsonrpc":"2.0","id":100,"result":null}"#.to_string())
                .unwrap();
        assert!(null_response.is_response());
        assert!(null_response.requires_session_id());
    }

    #[test]
    fn incoming_http_message_parse_rejects_batch_and_invalid_json() {
        let batch = IncomingHttpMessage::parse(r#"[{"jsonrpc":"2.0"}]"#.to_string()).unwrap_err();
        assert!(matches!(
            batch,
            HttpTransportError::NotImplemented {
                message: "batch JSON-RPC requests are not supported",
                source: None,
            }
        ));

        let invalid = IncomingHttpMessage::parse("{".to_string()).unwrap_err();
        assert!(matches!(
            invalid,
            HttpTransportError::BadRequest {
                message: "invalid JSON-RPC payload",
                source: Some(_),
            }
        ));
    }

    #[test]
    fn incoming_http_message_session_id_helpers_handle_valid_and_invalid_values() {
        let message = IncomingHttpMessage::parse(
            r#"{"jsonrpc":"2.0","id":1,"method":"session/prompt","params":{"sessionId":"session-1"}}"#
                .to_string(),
        )
        .unwrap();
        assert_eq!(message.params_session_id().unwrap(), Some(session_id()));

        let invalid = IncomingHttpMessage::parse(
            r#"{"jsonrpc":"2.0","id":1,"method":"session/prompt","params":{"sessionId":"bad.session"}}"#
                .to_string(),
        )
        .unwrap();
        assert!(matches!(
            invalid.params_session_id(),
            Err(HttpTransportError::BadRequest {
                message: "invalid sessionId in request body",
                source: Some(_),
            })
        ));
    }

    #[test]
    fn outgoing_http_message_extracts_session_ids() {
        let outbound = OutgoingHttpMessage::parse(
            r#"{"jsonrpc":"2.0","id":1,"params":{"sessionId":"session-1"}}"#,
        )
        .unwrap();
        assert_eq!(outbound.params_session_id(), Some(session_id()));

        let response = OutgoingHttpMessage::parse(
            r#"{"jsonrpc":"2.0","id":2,"result":{"sessionId":"session-1"}}"#,
        )
        .unwrap();
        assert_eq!(response.result_session_id(), Some(session_id()));
    }

    #[test]
    fn route_live_frame_keeps_same_session_notifications_on_post_stream() {
        let frame = SseFrame::json(
            r#"{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"session-1"}}"#
                .to_string(),
        );
        let parsed = OutgoingHttpMessage::parse(frame_json(&frame)).unwrap();
        let request_id = RequestId::Number(1);
        let request_session_id = session_id();
        let (live_tx, mut live_rx) = mpsc::channel(HTTP_CHANNEL_CAPACITY);
        let (get_tx, mut get_rx) = mpsc::channel(HTTP_CHANNEL_CAPACITY);
        let mut get_listeners = HashMap::new();
        get_listeners.insert(request_session_id.clone(), vec![get_tx]);

        let outcome = route_live_frame(
            &frame,
            Some(&parsed),
            &request_id,
            Some(&request_session_id),
            &live_tx,
            &mut get_listeners,
        );

        assert!(matches!(outcome, LiveFrameOutcome::Keep));
        match live_rx.try_recv().unwrap() {
            SseFrame::Json { json, .. } => assert!(json.contains(r#""sessionId":"session-1""#)),
            SseFrame::Empty { .. } => panic!("expected JSON SSE frame"),
        }
        assert!(matches!(
            get_rx.try_recv(),
            Err(tokio::sync::mpsc::error::TryRecvError::Empty)
        ));
    }

    #[test]
    fn route_live_frame_sends_other_session_notifications_to_get_listeners() {
        let frame = SseFrame::json(
            r#"{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"session-2"}}"#
                .to_string(),
        );
        let parsed = OutgoingHttpMessage::parse(frame_json(&frame)).unwrap();
        let request_id = RequestId::Number(1);
        let request_session_id = session_id();
        let other_session_id = acp_nats::AcpSessionId::new("session-2").unwrap();
        let (live_tx, mut live_rx) = mpsc::channel(HTTP_CHANNEL_CAPACITY);
        let (get_tx, mut get_rx) = mpsc::channel(HTTP_CHANNEL_CAPACITY);
        let mut get_listeners = HashMap::new();
        get_listeners.insert(other_session_id, vec![get_tx]);

        let outcome = route_live_frame(
            &frame,
            Some(&parsed),
            &request_id,
            Some(&request_session_id),
            &live_tx,
            &mut get_listeners,
        );

        assert!(matches!(outcome, LiveFrameOutcome::Keep));
        assert!(matches!(
            live_rx.try_recv(),
            Err(tokio::sync::mpsc::error::TryRecvError::Empty)
        ));
        match get_rx.try_recv().unwrap() {
            SseFrame::Json { json, .. } => assert!(json.contains(r#""sessionId":"session-2""#)),
            SseFrame::Empty { .. } => panic!("expected JSON SSE frame"),
        }
    }

    #[test]
    fn route_buffered_frame_sends_other_session_notifications_to_get_listeners() {
        let frame = SseFrame::json(
            r#"{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"session-2"}}"#
                .to_string(),
        );
        let parsed = OutgoingHttpMessage::parse(frame_json(&frame)).unwrap();
        let request_id = RequestId::Number(1);
        let other_session_id = acp_nats::AcpSessionId::new("session-2").unwrap();
        let (get_tx, mut get_rx) = mpsc::channel(HTTP_CHANNEL_CAPACITY);
        let mut events = Vec::new();
        let mut get_listeners = HashMap::new();
        get_listeners.insert(other_session_id, vec![get_tx]);

        let outcome = route_buffered_frame(
            &frame,
            Some(&parsed),
            &request_id,
            &mut events,
            &mut get_listeners,
        );

        assert!(matches!(outcome, BufferedFrameOutcome::Routed));
        assert!(events.is_empty());
        match get_rx.try_recv().unwrap() {
            SseFrame::Json { json, .. } => assert!(json.contains(r#""sessionId":"session-2""#)),
            SseFrame::Empty { .. } => panic!("expected JSON SSE frame"),
        }
    }

    #[test]
    fn dispatch_to_get_listeners_drops_full_listener() {
        let session_id = session_id();
        let mut get_listeners = HashMap::new();
        let (listener_tx, mut listener_rx) = mpsc::channel(1);
        listener_tx
            .try_send(SseFrame::json(
                r#"{"jsonrpc":"2.0","method":"session/update"}"#.to_string(),
            ))
            .unwrap();
        get_listeners.insert(session_id.clone(), vec![listener_tx]);

        let frame = SseFrame::json(r#"{"jsonrpc":"2.0","method":"session/update"}"#.to_string());
        let outcome = dispatch_to_get_listeners(&frame, &session_id, &mut get_listeners);

        assert!(matches!(outcome, ListenerDispatch::Dropped));
        assert!(!get_listeners.contains_key(&session_id));
        assert!(matches!(listener_rx.try_recv(), Ok(SseFrame::Json { .. })));
        assert!(matches!(
            listener_rx.try_recv(),
            Err(tokio::sync::mpsc::error::TryRecvError::Disconnected)
        ));
    }

    #[test]
    fn dispatch_to_get_listeners_delivers_each_message_on_only_one_stream() {
        let session_id = session_id();
        let mut get_listeners = HashMap::new();
        let (first_tx, mut first_rx) = mpsc::channel(HTTP_CHANNEL_CAPACITY);
        let (second_tx, mut second_rx) = mpsc::channel(HTTP_CHANNEL_CAPACITY);
        get_listeners.insert(session_id.clone(), vec![first_tx, second_tx]);

        let frame = SseFrame::json(
            r#"{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"session-1"}}"#
                .to_string(),
        );
        let outcome = dispatch_to_get_listeners(&frame, &session_id, &mut get_listeners);

        assert!(matches!(outcome, ListenerDispatch::Delivered));
        assert!(matches!(first_rx.try_recv(), Ok(SseFrame::Json { .. })));
        assert!(matches!(
            second_rx.try_recv(),
            Err(tokio::sync::mpsc::error::TryRecvError::Empty)
        ));
    }

    #[test]
    fn header_validators_enforce_content_negotiation() {
        let valid_post = post_headers();
        assert!(validate_post_headers(&valid_post).is_ok());

        let mut valid_post_with_charset = post_headers();
        valid_post_with_charset.insert(
            CONTENT_TYPE,
            HeaderValue::from_static("application/json; charset=utf-8"),
        );
        assert!(validate_post_headers(&valid_post_with_charset).is_ok());

        let mut bad_content_type = valid_post.clone();
        bad_content_type.insert(CONTENT_TYPE, HeaderValue::from_static("text/plain"));
        assert!(matches!(
            validate_post_headers(&bad_content_type),
            Err(HttpTransportError::UnsupportedMediaType {
                message: "Content-Type must be application/json",
                source: None,
            })
        ));

        let mut bad_accept = HeaderMap::new();
        bad_accept.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        bad_accept.insert(ACCEPT, HeaderValue::from_static("application/json"));
        assert!(matches!(
            validate_post_headers(&bad_accept),
            Err(HttpTransportError::NotAcceptable {
                message: "Accept must include application/json and text/event-stream",
                source: None,
            })
        ));

        let valid_get = get_headers();
        assert!(validate_get_headers(&valid_get).is_ok());

        let mut valid_post_with_q = HeaderMap::new();
        valid_post_with_q.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        valid_post_with_q.insert(
            ACCEPT,
            HeaderValue::from_static("application/json;q=0.9, text/event-stream"),
        );
        assert!(validate_post_headers(&valid_post_with_q).is_ok());

        let mut valid_get_with_q = HeaderMap::new();
        valid_get_with_q.insert(ACCEPT, HeaderValue::from_static("text/event-stream; q=0.5"));
        assert!(validate_get_headers(&valid_get_with_q).is_ok());

        let mut invalid_get = HeaderMap::new();
        invalid_get.insert(ACCEPT, HeaderValue::from_static("application/json"));
        assert!(matches!(
            validate_get_headers(&invalid_get),
            Err(HttpTransportError::NotAcceptable {
                message: "Accept must include text/event-stream",
                source: None,
            })
        ));
    }

    #[test]
    fn validate_origin_allows_loopback_and_matching_hosts() {
        let mut loopback = HeaderMap::new();
        loopback.insert(ORIGIN, HeaderValue::from_static("http://localhost:3000"));
        assert!(validate_origin(&loopback, IpAddr::V4(Ipv4Addr::LOCALHOST)).is_ok());

        let mut proxied = HeaderMap::new();
        proxied.insert(ORIGIN, HeaderValue::from_static("https://example.com"));
        proxied.insert(HOST, HeaderValue::from_static("example.com"));
        assert!(validate_origin(&proxied, IpAddr::V4(Ipv4Addr::UNSPECIFIED)).is_ok());
    }

    #[test]
    fn validate_origin_rejects_invalid_and_remote_origins() {
        let mut invalid = HeaderMap::new();
        invalid.insert(ORIGIN, HeaderValue::from_static("not a uri"));
        assert!(matches!(
            validate_origin(&invalid, IpAddr::V4(Ipv4Addr::LOCALHOST)),
            Err(HttpTransportError::Forbidden {
                message: "invalid Origin header",
                source: Some(_),
            })
        ));

        let mut remote = HeaderMap::new();
        remote.insert(ORIGIN, HeaderValue::from_static("https://evil.example"));
        assert!(matches!(
            validate_origin(&remote, IpAddr::V4(Ipv4Addr::LOCALHOST)),
            Err(HttpTransportError::Forbidden {
                message: "Origin is not allowed",
                source: None,
            })
        ));
    }

    #[test]
    fn validate_http_context_enforces_connection_and_session_rules() {
        let initialize = IncomingHttpMessage::parse(
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":0}}"#
                .to_string(),
        )
        .unwrap();
        let connection_id = AcpConnectionId::new();
        let session_id = session_id();

        assert!(validate_http_context(&initialize, None, None).is_ok());
        assert!(matches!(
            validate_http_context(&initialize, Some(&connection_id), None),
            Err(HttpTransportError::BadRequest {
                message: "initialize must not include Acp-Connection-Id",
                source: None,
            })
        ));

        let initialized =
            IncomingHttpMessage::parse(r#"{"jsonrpc":"2.0","method":"initialized"}"#.to_string())
                .unwrap();
        assert!(matches!(
            validate_http_context(&initialized, None, None),
            Err(HttpTransportError::BadRequest {
                message: "missing Acp-Connection-Id header",
                source: None,
            })
        ));

        let prompt = IncomingHttpMessage::parse(
            r#"{"jsonrpc":"2.0","id":2,"method":"session/prompt","params":{"sessionId":"session-1"}}"#
                .to_string(),
        )
        .unwrap();
        assert!(matches!(
            validate_http_context(&prompt, Some(&connection_id), None),
            Err(HttpTransportError::BadRequest {
                message: "missing Acp-Session-Id header",
                source: None,
            })
        ));

        let mismatched = IncomingHttpMessage::parse(
            r#"{"jsonrpc":"2.0","id":2,"method":"session/prompt","params":{"sessionId":"session-2"}}"#
                .to_string(),
        )
        .unwrap();
        assert!(matches!(
            validate_http_context(&mismatched, Some(&connection_id), Some(&session_id)),
            Err(HttpTransportError::BadRequest {
                message: "Acp-Session-Id header does not match request body sessionId",
                source: None,
            })
        ));
    }

    #[test]
    fn validate_protocol_version_header_allows_missing_and_rejects_mismatches() {
        assert!(validate_protocol_version_header(None, None).is_ok());
        assert!(validate_protocol_version_header(Some(&ProtocolVersion::V0), None).is_ok());
        assert!(validate_protocol_version_header(None, Some(&ProtocolVersion::V0)).is_ok());
        assert!(
            validate_protocol_version_header(
                Some(&ProtocolVersion::V0),
                Some(&ProtocolVersion::V0)
            )
            .is_ok()
        );

        assert!(matches!(
            validate_protocol_version_header(
                Some(&ProtocolVersion::V1),
                Some(&ProtocolVersion::V0)
            ),
            Err(HttpTransportError::BadRequest {
                message: "Acp-Protocol-Version header does not match initialized protocol version",
                source: None,
            })
        ));
    }

    #[test]
    fn header_parsers_and_websocket_detection_handle_valid_and_invalid_values() {
        let connection_id = AcpConnectionId::new();
        let session_id = session_id();
        let mut headers = HeaderMap::new();
        headers.insert(
            ACP_CONNECTION_ID_HEADER,
            HeaderValue::from_str(&connection_id.to_string()).unwrap(),
        );
        headers.insert(
            ACP_SESSION_ID_HEADER,
            HeaderValue::from_str(session_id.as_str()).unwrap(),
        );
        headers.insert(ACP_PROTOCOL_VERSION_HEADER, HeaderValue::from_static("0"));
        headers.insert("upgrade", HeaderValue::from_static("websocket"));

        assert_eq!(
            parse_connection_id_header(&headers).unwrap(),
            Some(connection_id.clone())
        );
        assert_eq!(parse_session_id_header(&headers).unwrap(), Some(session_id));
        assert_eq!(
            parse_protocol_version_header(&headers).unwrap(),
            Some(ProtocolVersion::V0)
        );
        assert!(is_websocket_request(&headers));

        headers.insert(
            ACP_CONNECTION_ID_HEADER,
            HeaderValue::from_static("not-a-uuid"),
        );
        assert!(matches!(
            parse_connection_id_header(&headers),
            Err(HttpTransportError::BadRequest {
                message: "invalid Acp-Connection-Id header",
                source: Some(_),
            })
        ));

        headers.insert(
            ACP_CONNECTION_ID_HEADER,
            HeaderValue::from_str(&connection_id.to_string()).unwrap(),
        );
        headers.insert(
            ACP_PROTOCOL_VERSION_HEADER,
            HeaderValue::from_static("not-a-version"),
        );
        assert!(matches!(
            parse_protocol_version_header(&headers),
            Err(HttpTransportError::BadRequest {
                message: "invalid Acp-Protocol-Version header",
                source: Some(_),
            })
        ));
    }

    #[tokio::test]
    async fn http_post_returns_accepted_for_notifications() {
        let (state, mut manager_rx) = test_state();
        let connection_id = AcpConnectionId::new();
        let expected_connection_id = connection_id.clone();

        tokio::spawn(async move {
            match manager_rx.recv().await.unwrap() {
                ManagerRequest::HttpPost {
                    connection_id: Some(actual_connection_id),
                    session_id: None,
                    message,
                    response,
                    ..
                } => {
                    assert_eq!(actual_connection_id, expected_connection_id);
                    assert!(message.is_notification());
                    let _ = response.send(Ok(HttpPostOutcome::Accepted));
                }
                _ => panic!("unexpected manager request"),
            }
        });

        let mut headers = post_headers();
        headers.insert(
            ACP_CONNECTION_ID_HEADER,
            HeaderValue::from_str(&connection_id.to_string()).unwrap(),
        );

        let response = http_post(
            headers,
            state,
            r#"{"jsonrpc":"2.0","method":"initialized"}"#.to_string(),
        )
        .await
        .unwrap();

        assert_eq!(response.status(), StatusCode::ACCEPTED);
    }

    #[tokio::test]
    async fn http_post_accepts_null_result_responses() {
        let (state, mut manager_rx) = test_state();
        let connection_id = AcpConnectionId::new();
        let session_id = session_id();
        let expected_connection_id = connection_id.clone();
        let expected_session_id = session_id.clone();

        tokio::spawn(async move {
            match manager_rx.recv().await.unwrap() {
                ManagerRequest::HttpPost {
                    connection_id: Some(actual_connection_id),
                    session_id: Some(actual_session_id),
                    message,
                    response,
                    ..
                } => {
                    assert_eq!(actual_connection_id, expected_connection_id);
                    assert_eq!(actual_session_id, expected_session_id);
                    assert!(message.is_response());
                    let _ = response.send(Ok(HttpPostOutcome::Accepted));
                }
                _ => panic!("unexpected manager request"),
            }
        });

        let mut headers = post_headers();
        headers.insert(
            ACP_CONNECTION_ID_HEADER,
            HeaderValue::from_str(&connection_id.to_string()).unwrap(),
        );
        headers.insert(
            ACP_SESSION_ID_HEADER,
            HeaderValue::from_str(session_id.as_str()).unwrap(),
        );

        let response = http_post(
            headers,
            state,
            r#"{"jsonrpc":"2.0","id":1,"result":null}"#.to_string(),
        )
        .await
        .unwrap();

        assert_eq!(response.status(), StatusCode::ACCEPTED);
    }

    #[tokio::test]
    async fn http_post_returns_buffered_sse_with_session_headers() {
        let (state, mut manager_rx) = test_state();
        let connection_id = AcpConnectionId::new();
        let session_id = session_id();
        let event = json!({
            "jsonrpc": "2.0",
            "id": 2,
            "result": { "sessionId": session_id.as_str() }
        });
        let expected_event = event.clone();

        let expected_connection_id = connection_id.clone();
        let expected_session_id = session_id.clone();
        tokio::spawn(async move {
            match manager_rx.recv().await.unwrap() {
                ManagerRequest::HttpPost {
                    connection_id: Some(actual_connection_id),
                    session_id: None,
                    message,
                    response,
                    ..
                } => {
                    assert_eq!(actual_connection_id, expected_connection_id.clone());
                    assert!(message.is_session_new());
                    let _ = response.send(Ok(HttpPostOutcome::Buffered {
                        connection_id: expected_connection_id,
                        protocol_version: Some(ProtocolVersion::V0),
                        session_id: Some(expected_session_id),
                        events: vec![SseFrame::json(event.to_string())],
                    }));
                }
                _ => panic!("unexpected manager request"),
            }
        });

        let mut headers = post_headers();
        headers.insert(
            ACP_CONNECTION_ID_HEADER,
            HeaderValue::from_str(&connection_id.to_string()).unwrap(),
        );

        let response = http_post(
            headers,
            state,
            r#"{"jsonrpc":"2.0","id":2,"method":"session/new","params":{"cwd":".","mcpServers":[]}}"#
                .to_string(),
        )
        .await
        .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers().get(ACP_CONNECTION_ID_HEADER).unwrap(),
            HeaderValue::from_str(&connection_id.to_string()).unwrap()
        );
        assert_eq!(
            response.headers().get(ACP_SESSION_ID_HEADER).unwrap(),
            HeaderValue::from_str(session_id.as_str()).unwrap()
        );
        assert_eq!(
            response.headers().get(ACP_PROTOCOL_VERSION_HEADER).unwrap(),
            HeaderValue::from_static("0")
        );
        assert_eq!(json_event_body(response).await, vec![expected_event]);
    }

    #[tokio::test]
    async fn http_get_and_delete_round_trip_through_manager() {
        let (state, mut manager_rx) = test_state();
        let connection_id = AcpConnectionId::new();
        let session_id = session_id();
        let expected_connection_id = connection_id.clone();
        let expected_session_id = session_id.clone();

        tokio::spawn(async move {
            match manager_rx.recv().await.unwrap() {
                ManagerRequest::HttpGet {
                    connection_id: actual_connection_id,
                    session_id: actual_session_id,
                    protocol_version,
                    response,
                } => {
                    assert_eq!(actual_connection_id, expected_connection_id.clone());
                    assert_eq!(actual_session_id, expected_session_id);
                    assert_eq!(protocol_version, None);
                    let (stream_tx, stream_rx) = mpsc::channel(HTTP_CHANNEL_CAPACITY);
                    let _ = stream_tx.try_send(SseFrame::json(
                        json!({
                            "jsonrpc": "2.0",
                            "method": "session/update",
                            "params": { "sessionId": "session-1" }
                        })
                        .to_string(),
                    ));
                    drop(stream_tx);
                    let _ = response.send(Ok(HttpGetOutcome {
                        protocol_version: Some(ProtocolVersion::V0),
                        stream: stream_rx,
                    }));
                }
                _ => panic!("unexpected manager request"),
            }
            match manager_rx.recv().await.unwrap() {
                ManagerRequest::HttpDelete {
                    connection_id: actual_connection_id,
                    protocol_version,
                    response,
                } => {
                    assert_eq!(actual_connection_id, expected_connection_id);
                    assert_eq!(protocol_version, None);
                    let _ = response.send(Ok(Some(ProtocolVersion::V0)));
                }
                _ => panic!("unexpected manager request"),
            }
        });

        let mut headers = get_headers();
        headers.insert(
            ACP_CONNECTION_ID_HEADER,
            HeaderValue::from_str(&connection_id.to_string()).unwrap(),
        );
        headers.insert(
            ACP_SESSION_ID_HEADER,
            HeaderValue::from_str(session_id.as_str()).unwrap(),
        );

        let response = http_get(headers, state.clone()).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers().get(ACP_CONNECTION_ID_HEADER).unwrap(),
            HeaderValue::from_str(&connection_id.to_string()).unwrap()
        );
        assert_eq!(
            response.headers().get(ACP_SESSION_ID_HEADER).unwrap(),
            HeaderValue::from_str(session_id.as_str()).unwrap()
        );
        assert_eq!(
            response.headers().get(ACP_PROTOCOL_VERSION_HEADER).unwrap(),
            HeaderValue::from_static("0")
        );
        assert_eq!(
            response.headers().get(X_ACCEL_BUFFERING_HEADER).unwrap(),
            HeaderValue::from_static("no")
        );
        assert_eq!(
            json_event_body(response).await,
            vec![json!({
                "jsonrpc": "2.0",
                "method": "session/update",
                "params": { "sessionId": "session-1" }
            })]
        );

        let mut delete_headers = HeaderMap::new();
        delete_headers.insert(
            ACP_CONNECTION_ID_HEADER,
            HeaderValue::from_str(&connection_id.to_string()).unwrap(),
        );

        let response = http_delete(delete_headers, state).await.unwrap();
        assert_eq!(response.status(), StatusCode::ACCEPTED);
        assert_eq!(
            response.headers().get(ACP_PROTOCOL_VERSION_HEADER).unwrap(),
            HeaderValue::from_static("0")
        );
    }

    #[tokio::test]
    async fn get_rejects_incomplete_websocket_upgrade() {
        let (state, _manager_rx) = test_state();
        let request = HttpRequest::builder()
            .method("GET")
            .uri("/acp")
            .header("upgrade", "websocket")
            .body(Body::empty())
            .unwrap();

        let response = get(State(state), request).await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn post_rejects_disallowed_origin_before_processing() {
        let (state, _manager_rx) = test_state();
        let mut headers = post_headers();
        headers.insert(ORIGIN, HeaderValue::from_static("https://evil.example"));

        let response = post(
            headers,
            State(state),
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":0}}"#
                .to_string(),
        )
        .await;

        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        assert_eq!(
            String::from_utf8(bytes.to_vec()).unwrap(),
            "Origin is not allowed"
        );
    }

    #[tokio::test]
    async fn get_rejects_websocket_upgrade_with_disallowed_origin() {
        let (state, _manager_rx) = test_state();
        let request = HttpRequest::builder()
            .method("GET")
            .uri("/acp")
            .header("upgrade", "websocket")
            .header(ORIGIN, "https://evil.example")
            .body(Body::empty())
            .unwrap();

        let response = get(State(state), request).await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn process_manager_request_rejects_invalid_or_unknown_http_targets() {
        let nats_client = AdvancedMockNatsClient::new();
        let js_client = MockJs::new();
        let config = test_config();
        let mut http_connections = HashMap::new();
        let mut websocket_handles = Vec::new();
        let mut http_connection_handles = Vec::new();
        let (_shutdown_tx, shutdown_rx) = watch::channel(false);

        let (post_response_tx, post_response_rx) = oneshot::channel();
        let post_message =
            IncomingHttpMessage::parse(r#"{"jsonrpc":"2.0","method":"initialized"}"#.to_string())
                .unwrap();
        process_manager_request(
            ManagerRequest::HttpPost {
                connection_id: None,
                protocol_version: None,
                session_id: None,
                message: post_message,
                response: post_response_tx,
                shutdown_rx: shutdown_rx.clone(),
            },
            &mut http_connections,
            &mut websocket_handles,
            &mut http_connection_handles,
            &nats_client,
            &js_client,
            &config,
        )
        .await;
        assert!(matches!(
            post_response_rx.await.unwrap(),
            Err(HttpTransportError::BadRequest {
                message: "missing Acp-Connection-Id header",
                source: None,
            })
        ));

        let unknown_connection_id = AcpConnectionId::new();
        let (get_response_tx, get_response_rx) = oneshot::channel();
        process_manager_request(
            ManagerRequest::HttpGet {
                connection_id: unknown_connection_id.clone(),
                session_id: session_id(),
                protocol_version: None,
                response: get_response_tx,
            },
            &mut http_connections,
            &mut websocket_handles,
            &mut http_connection_handles,
            &nats_client,
            &js_client,
            &config,
        )
        .await;
        assert!(matches!(
            get_response_rx.await.unwrap(),
            Err(HttpTransportError::NotFound {
                message: "unknown ACP connection",
                source: None,
            })
        ));

        let (delete_response_tx, delete_response_rx) = oneshot::channel();
        process_manager_request(
            ManagerRequest::HttpDelete {
                connection_id: unknown_connection_id,
                protocol_version: None,
                response: delete_response_tx,
            },
            &mut http_connections,
            &mut websocket_handles,
            &mut http_connection_handles,
            &nats_client,
            &js_client,
            &config,
        )
        .await;
        assert!(matches!(
            delete_response_rx.await.unwrap(),
            Err(HttpTransportError::NotFound {
                message: "unknown ACP connection",
                source: None,
            })
        ));
    }

    #[tokio::test]
    async fn process_manager_request_prunes_closed_http_connections() {
        let nats_client = AdvancedMockNatsClient::new();
        let js_client = MockJs::new();
        let config = test_config();
        let mut http_connections = HashMap::new();
        let mut websocket_handles = Vec::new();
        let mut http_connection_handles = Vec::new();

        let stale_connection_id = AcpConnectionId::new();
        let (command_tx, command_rx) = mpsc::unbounded_channel();
        drop(command_rx);
        http_connections.insert(stale_connection_id, HttpConnectionHandle { command_tx });

        let unknown_connection_id = AcpConnectionId::new();
        let (response_tx, response_rx) = oneshot::channel();
        process_manager_request(
            ManagerRequest::HttpGet {
                connection_id: unknown_connection_id,
                session_id: session_id(),
                protocol_version: None,
                response: response_tx,
            },
            &mut http_connections,
            &mut websocket_handles,
            &mut http_connection_handles,
            &nats_client,
            &js_client,
            &config,
        )
        .await;

        assert!(http_connections.is_empty());
        assert!(matches!(
            response_rx.await.unwrap(),
            Err(HttpTransportError::NotFound {
                message: "unknown ACP connection",
                source: None,
            })
        ));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn process_manager_request_tracks_http_connection_tasks() {
        let local = tokio::task::LocalSet::new();
        local
            .run_until(async {
                let nats_client = AdvancedMockNatsClient::new();
                let _injector = nats_client.inject_messages();
                nats_client.hang_next_request();

                let js_client = MockJs::new();
                let config = test_config();
                let mut http_connections = HashMap::new();
                let mut websocket_handles = Vec::new();
                let mut http_connection_handles = Vec::new();
                let (shutdown_tx, shutdown_rx) = watch::channel(false);

                let (response_tx, response_rx) = oneshot::channel();
                let initialize = IncomingHttpMessage::parse(
                    r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":0}}"#
                        .to_string(),
                )
                .unwrap();

                process_manager_request(
                    ManagerRequest::HttpPost {
                        connection_id: None,
                        protocol_version: None,
                        session_id: None,
                        message: initialize,
                        response: response_tx,
                        shutdown_rx,
                    },
                    &mut http_connections,
                    &mut websocket_handles,
                    &mut http_connection_handles,
                    &nats_client,
                    &js_client,
                    &config,
                )
                .await;

                assert_eq!(http_connections.len(), 1);
                assert_eq!(http_connection_handles.len(), 1);
                assert!(!http_connection_handles[0].is_finished());
                assert!(matches!(
                    response_rx.await.unwrap(),
                    Ok(HttpPostOutcome::Live { .. })
                ));

                let _ = shutdown_tx.send(true);
                tokio::time::timeout(std::time::Duration::from_secs(2), async {
                    while !http_connection_handles[0].is_finished() {
                        tokio::task::yield_now().await;
                    }
                })
                .await
                .expect("HTTP connection task did not finish after shutdown");
            })
            .await;
    }
}
