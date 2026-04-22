use crate::acp_connection_id::{AcpConnectionId, AcpConnectionIdError};
use crate::connection;
use crate::constants::{ACP_CONNECTION_ID_HEADER, ACP_SESSION_ID_HEADER, X_ACCEL_BUFFERING_HEADER};
use acp_nats::{StdJsonSerialize, agent::Bridge, client, spawn_notification_forwarder};
use agent_client_protocol::{AgentSideConnection, RequestId, SessionNotification};
use axum::extract::FromRequestParts;
use axum::extract::Request;
use axum::extract::State;
use axum::extract::ws::{WebSocket, WebSocketUpgrade};
use axum::http::header::{ACCEPT, CONTENT_TYPE};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::sse::{Event, Sse};
use axum::response::{IntoResponse, Response};
use futures_util::stream;
use serde::Deserialize;
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::convert::Infallible;
use std::rc::Rc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
use tokio::sync::{mpsc, oneshot, watch};
use tracing::{error, info, warn};
use trogon_std::time::SystemClock;

type SseSender = mpsc::UnboundedSender<SseFrame>;
type SseReceiver = mpsc::UnboundedReceiver<SseFrame>;

#[derive(Clone)]
pub struct AppState {
    pub manager_tx: mpsc::UnboundedSender<ManagerRequest>,
    pub shutdown_tx: watch::Sender<bool>,
}

pub struct ConnectionRequest {
    pub connection_id: AcpConnectionId,
    pub socket: WebSocket,
    pub shutdown_rx: watch::Receiver<bool>,
}

pub enum ManagerRequest {
    WebSocket(ConnectionRequest),
    HttpPost {
        connection_id: Option<AcpConnectionId>,
        session_id: Option<acp_nats::AcpSessionId>,
        message: IncomingHttpMessage,
        response: oneshot::Sender<Result<HttpPostOutcome, HttpTransportError>>,
        shutdown_rx: watch::Receiver<bool>,
    },
    HttpGet {
        connection_id: AcpConnectionId,
        session_id: acp_nats::AcpSessionId,
        response: oneshot::Sender<Result<SseReceiver, HttpTransportError>>,
    },
    HttpDelete {
        connection_id: AcpConnectionId,
        response: oneshot::Sender<Result<(), HttpTransportError>>,
    },
}

#[derive(Debug)]
pub enum HttpPostOutcome {
    Accepted,
    Live {
        connection_id: AcpConnectionId,
        session_id: Option<acp_nats::AcpSessionId>,
        stream: SseReceiver,
    },
    Buffered {
        connection_id: AcpConnectionId,
        session_id: Option<acp_nats::AcpSessionId>,
        events: Vec<SseFrame>,
    },
}

#[derive(Debug)]
pub enum HttpTransportError {
    BadRequest(&'static str),
    NotFound(&'static str),
    Conflict(&'static str),
    UnsupportedMediaType(&'static str),
    NotAcceptable(&'static str),
    NotImplemented(&'static str),
    Internal(&'static str),
}

impl HttpTransportError {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            Self::BadRequest(message) => (StatusCode::BAD_REQUEST, message),
            Self::NotFound(message) => (StatusCode::NOT_FOUND, message),
            Self::Conflict(message) => (StatusCode::CONFLICT, message),
            Self::UnsupportedMediaType(message) => (StatusCode::UNSUPPORTED_MEDIA_TYPE, message),
            Self::NotAcceptable(message) => (StatusCode::NOT_ACCEPTABLE, message),
            Self::NotImplemented(message) => (StatusCode::NOT_IMPLEMENTED, message),
            Self::Internal(message) => (StatusCode::INTERNAL_SERVER_ERROR, message),
        };

        (status, message).into_response()
    }
}

impl std::fmt::Display for HttpTransportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BadRequest(message)
            | Self::NotFound(message)
            | Self::Conflict(message)
            | Self::UnsupportedMediaType(message)
            | Self::NotAcceptable(message)
            | Self::NotImplemented(message)
            | Self::Internal(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for HttpTransportError {}

#[derive(Debug)]
pub struct HttpConnectionHandle {
    pub command_tx: mpsc::UnboundedSender<HttpConnectionCommand>,
}

#[derive(Debug)]
pub enum HttpConnectionCommand {
    Post {
        session_id: Option<acp_nats::AcpSessionId>,
        message: IncomingHttpMessage,
        response: oneshot::Sender<Result<HttpPostOutcome, HttpTransportError>>,
    },
    AttachListener {
        session_id: acp_nats::AcpSessionId,
        response: oneshot::Sender<Result<SseReceiver, HttpTransportError>>,
    },
    Close {
        response: oneshot::Sender<Result<(), HttpTransportError>>,
    },
}

#[derive(Clone, Debug)]
pub(crate) enum SseFrame {
    Json(String),
}

impl SseFrame {
    fn into_event(self) -> Event {
        match self {
            Self::Json(json) => Event::default().data(json),
        }
    }
}

#[derive(Debug)]
enum PendingRequest {
    Live {
        request_id: RequestId,
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
    pub result: Option<Value>,
    pub error: Option<Value>,
    #[serde(skip)]
    pub raw: String,
}

impl IncomingHttpMessage {
    pub fn parse(raw: String) -> Result<Self, HttpTransportError> {
        let trimmed = raw.trim_start();
        if trimmed.starts_with('[') {
            return Err(HttpTransportError::NotImplemented(
                "batch JSON-RPC requests are not supported",
            ));
        }

        let mut parsed = serde_json::from_str::<Self>(&raw)
            .map_err(|_| HttpTransportError::BadRequest("invalid JSON-RPC payload"))?;
        parsed.raw = raw;
        Ok(parsed)
    }

    fn is_request(&self) -> bool {
        self.id.is_some() && self.method.is_some()
    }

    fn is_notification(&self) -> bool {
        self.id.is_none() && self.method.is_some()
    }

    fn is_response(&self) -> bool {
        self.id.is_some()
            && self.method.is_none()
            && (self.result.is_some() || self.error.is_some())
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
            .map_err(|_| HttpTransportError::BadRequest("invalid sessionId in request body"))
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
}

pub async fn get(State(state): State<AppState>, request: Request) -> Response {
    if is_websocket_request(request.headers()) {
        let (mut parts, _body) = request.into_parts();
        match WebSocketUpgrade::from_request_parts(&mut parts, &state).await {
            Ok(ws) => websocket_response(ws, state),
            Err(_) => {
                HttpTransportError::BadRequest("invalid WebSocket upgrade request").into_response()
            }
        }
    } else {
        match http_get(request.headers().clone(), state).await {
            Ok(response) => response,
            Err(error) => error.into_response(),
        }
    }
}

pub async fn legacy_websocket_get(ws: WebSocketUpgrade, State(state): State<AppState>) -> Response {
    websocket_response(ws, state)
}

pub async fn post(headers: HeaderMap, State(state): State<AppState>, body: String) -> Response {
    match http_post(headers, state, body).await {
        Ok(response) => response,
        Err(error) => error.into_response(),
    }
}

pub async fn delete(headers: HeaderMap, State(state): State<AppState>) -> Response {
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
            .send(ManagerRequest::WebSocket(ConnectionRequest {
                connection_id,
                socket,
                shutdown_rx,
            }))
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
        return Err(HttpTransportError::BadRequest(
            "invalid JSON-RPC message shape",
        ));
    }

    let connection_id = parse_connection_id_header(&headers)?;
    let session_id = parse_session_id_header(&headers)?;

    validate_http_context(&message, connection_id.as_ref(), session_id.as_ref())?;

    let (response_tx, response_rx) = oneshot::channel();
    state
        .manager_tx
        .send(ManagerRequest::HttpPost {
            connection_id,
            session_id,
            message,
            response: response_tx,
            shutdown_rx: state.shutdown_tx.subscribe(),
        })
        .map_err(|_| HttpTransportError::Internal("connection manager is unavailable"))?;

    match response_rx
        .await
        .map_err(|_| HttpTransportError::Internal("connection manager dropped the request"))??
    {
        HttpPostOutcome::Accepted => Ok(StatusCode::ACCEPTED.into_response()),
        HttpPostOutcome::Live {
            connection_id,
            session_id,
            stream,
        } => Ok(build_sse_response(connection_id, session_id, stream)),
        HttpPostOutcome::Buffered {
            connection_id,
            session_id,
            events,
        } => Ok(build_buffered_sse_response(
            connection_id,
            session_id,
            events,
        )),
    }
}

async fn http_get(headers: HeaderMap, state: AppState) -> Result<Response, HttpTransportError> {
    validate_get_headers(&headers)?;

    let connection_id = parse_connection_id_header(&headers)?.ok_or(
        HttpTransportError::BadRequest("missing Acp-Connection-Id header"),
    )?;
    let session_id = parse_session_id_header(&headers)?.ok_or(HttpTransportError::BadRequest(
        "missing Acp-Session-Id header",
    ))?;

    let (response_tx, response_rx) = oneshot::channel();
    state
        .manager_tx
        .send(ManagerRequest::HttpGet {
            connection_id,
            session_id,
            response: response_tx,
        })
        .map_err(|_| HttpTransportError::Internal("connection manager is unavailable"))?;

    let stream = response_rx
        .await
        .map_err(|_| HttpTransportError::Internal("connection manager dropped the request"))??;

    let mut response = Sse::new(stream::unfold(stream, |mut stream| async move {
        stream
            .recv()
            .await
            .map(|item| (Ok::<Event, Infallible>(item.into_event()), stream))
    }))
    .into_response();
    response
        .headers_mut()
        .insert(X_ACCEL_BUFFERING_HEADER, HeaderValue::from_static("no"));
    Ok(response)
}

async fn http_delete(headers: HeaderMap, state: AppState) -> Result<Response, HttpTransportError> {
    let connection_id = parse_connection_id_header(&headers)?.ok_or(
        HttpTransportError::BadRequest("missing Acp-Connection-Id header"),
    )?;

    let (response_tx, response_rx) = oneshot::channel();
    state
        .manager_tx
        .send(ManagerRequest::HttpDelete {
            connection_id,
            response: response_tx,
        })
        .map_err(|_| HttpTransportError::Internal("connection manager is unavailable"))?;

    response_rx
        .await
        .map_err(|_| HttpTransportError::Internal("connection manager dropped the request"))??;

    Ok(StatusCode::ACCEPTED.into_response())
}

fn validate_post_headers(headers: &HeaderMap) -> Result<(), HttpTransportError> {
    match headers
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
    {
        Some(value) if value.eq_ignore_ascii_case("application/json") => {}
        _ => {
            return Err(HttpTransportError::UnsupportedMediaType(
                "Content-Type must be application/json",
            ));
        }
    }

    let accept = headers
        .get(ACCEPT)
        .and_then(|value| value.to_str().ok())
        .ok_or(HttpTransportError::NotAcceptable(
            "Accept must include application/json and text/event-stream",
        ))?;

    if !accept_contains(accept, "application/json") || !accept_contains(accept, "text/event-stream")
    {
        return Err(HttpTransportError::NotAcceptable(
            "Accept must include application/json and text/event-stream",
        ));
    }

    Ok(())
}

fn validate_get_headers(headers: &HeaderMap) -> Result<(), HttpTransportError> {
    let accept = headers
        .get(ACCEPT)
        .and_then(|value| value.to_str().ok())
        .ok_or(HttpTransportError::NotAcceptable(
            "Accept must include text/event-stream",
        ))?;

    if !accept_contains(accept, "text/event-stream") {
        return Err(HttpTransportError::NotAcceptable(
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
            return Err(HttpTransportError::BadRequest(
                "initialize must not include Acp-Connection-Id",
            ));
        }
        if session_id.is_some() {
            return Err(HttpTransportError::BadRequest(
                "initialize must not include Acp-Session-Id",
            ));
        }
        return Ok(());
    }

    if connection_id.is_none() {
        return Err(HttpTransportError::BadRequest(
            "missing Acp-Connection-Id header",
        ));
    }

    let body_session_id = message.params_session_id()?;
    if message.requires_session_id() && session_id.is_none() {
        return Err(HttpTransportError::BadRequest(
            "missing Acp-Session-Id header",
        ));
    }

    if let (Some(header_session_id), Some(body_session_id)) = (session_id, body_session_id.as_ref())
    {
        if header_session_id != body_session_id {
            return Err(HttpTransportError::BadRequest(
                "Acp-Session-Id header does not match request body sessionId",
            ));
        }
    }

    Ok(())
}

fn accept_contains(header: &str, expected: &str) -> bool {
    header
        .split(',')
        .map(str::trim)
        .any(|value| value.eq_ignore_ascii_case(expected))
}

fn parse_connection_id_header(
    headers: &HeaderMap,
) -> Result<Option<AcpConnectionId>, HttpTransportError> {
    headers
        .get(ACP_CONNECTION_ID_HEADER)
        .map(|value| {
            value
                .to_str()
                .map_err(|_| HttpTransportError::BadRequest("invalid Acp-Connection-Id header"))
                .and_then(|value| {
                    AcpConnectionId::parse(value).map_err(|error| match error {
                        AcpConnectionIdError::InvalidUuid(_) => {
                            HttpTransportError::BadRequest("invalid Acp-Connection-Id header")
                        }
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
                .map_err(|_| HttpTransportError::BadRequest("invalid Acp-Session-Id header"))
                .and_then(|value| {
                    acp_nats::AcpSessionId::new(value).map_err(|_| {
                        HttpTransportError::BadRequest("invalid Acp-Session-Id header")
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
    session_id: Option<acp_nats::AcpSessionId>,
    stream: SseReceiver,
) -> Response {
    let mut response = Sse::new(stream::unfold(stream, |mut stream| async move {
        stream
            .recv()
            .await
            .map(|item| (Ok::<Event, Infallible>(item.into_event()), stream))
    }))
    .into_response();
    set_transport_headers(response.headers_mut(), &connection_id, session_id.as_ref());
    response
        .headers_mut()
        .insert(X_ACCEL_BUFFERING_HEADER, HeaderValue::from_static("no"));
    response
}

fn build_buffered_sse_response(
    connection_id: AcpConnectionId,
    session_id: Option<acp_nats::AcpSessionId>,
    events: Vec<SseFrame>,
) -> Response {
    let stream = stream::iter(
        events
            .into_iter()
            .map(|item| Ok::<Event, Infallible>(item.into_event())),
    );
    let mut response = Sse::new(stream).into_response();
    set_transport_headers(response.headers_mut(), &connection_id, session_id.as_ref());
    response
        .headers_mut()
        .insert(X_ACCEL_BUFFERING_HEADER, HeaderValue::from_static("no"));
    response
}

fn set_transport_headers(
    headers: &mut HeaderMap,
    connection_id: &AcpConnectionId,
    session_id: Option<&acp_nats::AcpSessionId>,
) {
    headers.insert(
        ACP_CONNECTION_ID_HEADER,
        HeaderValue::from_str(&connection_id.to_string())
            .expect("generated ACP connection id must be a valid header value"),
    );
    if let Some(session_id) = session_id {
        headers.insert(
            ACP_SESSION_ID_HEADER,
            HeaderValue::from_str(session_id.as_str())
                .expect("generated ACP session id must be a valid header value"),
        );
    }
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

    let (input_tx, mut input_rx) = mpsc::unbounded_channel::<String>();
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

    let (output_tx, mut output_rx) = mpsc::unbounded_channel::<String>();
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
                    if output_tx.send(trimmed.to_string()).is_err() {
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
                    HttpConnectionCommand::Post { session_id, message, response } => {
                        if message.is_request() {
                            if pending_request.is_some() {
                                let _ = response.send(Err(HttpTransportError::Conflict(
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
                                let _ = input_tx.send(message.raw);
                                continue;
                            }

                            if let Some(session_id) = session_id.clone() {
                                sessions.insert(session_id);
                            }

                            let (stream_tx, stream_rx) = mpsc::unbounded_channel();
                            pending_request = Some(PendingRequest::Live {
                                request_id: message.id.clone().expect("request must have id"),
                                sender: stream_tx,
                            });
                            let _ = input_tx.send(message.raw);
                            let _ = response.send(Ok(HttpPostOutcome::Live {
                                connection_id: connection_id.clone(),
                                session_id,
                                stream: stream_rx,
                            }));
                            continue;
                        }

                        if let Some(session_id) = session_id.clone() {
                            sessions.insert(session_id);
                        }

                        if input_tx.send(message.raw).is_err() {
                            let _ = response.send(Err(HttpTransportError::Internal(
                                "failed to forward HTTP payload into ACP runtime",
                            )));
                            continue;
                        }

                        let _ = response.send(Ok(HttpPostOutcome::Accepted));
                    }
                    HttpConnectionCommand::AttachListener { session_id, response } => {
                        if !sessions.contains(&session_id) {
                            let _ = response.send(Err(HttpTransportError::NotFound(
                                "unknown ACP session",
                            )));
                            continue;
                        }

                        let (stream_tx, stream_rx) = mpsc::unbounded_channel();
                        get_listeners.entry(session_id).or_default().push(stream_tx);
                        let _ = response.send(Ok(stream_rx));
                    }
                    HttpConnectionCommand::Close { response } => {
                        let _ = response.send(Ok(()));
                        break;
                    }
                }
            }
            outbound = output_rx.recv() => {
                let Some(outbound) = outbound else {
                    break;
                };

                let frame = SseFrame::Json(outbound.clone());
                let parsed = OutgoingHttpMessage::parse(&outbound);

                if let Some(pending) = pending_request.as_mut() {
                    match pending {
                        PendingRequest::Live { request_id, sender, .. } => {
                            if sender.send(frame.clone()).is_err() {
                                pending_request = None;
                                continue;
                            }

                            if parsed.as_ref().and_then(|message| message.id.as_ref()) == Some(request_id) {
                                pending_request = None;
                            }
                            continue;
                        }
                        PendingRequest::Buffered { request_id, events, .. } => {
                            events.push(frame);
                            if parsed.as_ref().and_then(|message| message.id.as_ref()) == Some(request_id) {
                                let session_id = parsed.and_then(|message| message.result_session_id());
                                if let Some(session_id) = session_id.clone() {
                                    sessions.insert(session_id);
                                }
                                let events = std::mem::take(events);
                                if let Some(PendingRequest::Buffered { response, .. }) = pending_request.take() {
                                    let _ = response.send(Ok(HttpPostOutcome::Buffered {
                                        connection_id: connection_id.clone(),
                                        session_id,
                                        events,
                                    }));
                                }
                            }
                            continue;
                        }
                    }
                }

                let Some(session_id) = parsed.and_then(|message| message.params_session_id()) else {
                    continue;
                };

                let Some(listeners) = get_listeners.get_mut(&session_id) else {
                    continue;
                };

                listeners.retain(|listener| listener.send(frame.clone()).is_ok());
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
        let _ = response.send(Err(HttpTransportError::Internal(
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

    match request {
        ManagerRequest::WebSocket(request) => {
            websocket_handles.push(tokio::task::spawn_local(connection::handle(
                request.connection_id,
                request.socket,
                nats_client.clone(),
                js_client.clone(),
                config.clone(),
                request.shutdown_rx,
            )));
        }
        ManagerRequest::HttpPost {
            connection_id,
            session_id,
            message,
            response,
            shutdown_rx,
        } => {
            let connection_id = match connection_id {
                Some(connection_id) => connection_id,
                None => {
                    if !message.is_initialize() {
                        let _ = response.send(Err(HttpTransportError::BadRequest(
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
                    tokio::task::spawn_local(run_http_connection(
                        connection_id.clone(),
                        nats_client.clone(),
                        js_client.clone(),
                        config.clone(),
                        command_rx,
                        shutdown_rx,
                    ));
                    connection_id
                }
            };

            let Some(handle) = http_connections.get(&connection_id) else {
                let _ = response.send(Err(HttpTransportError::NotFound("unknown ACP connection")));
                return;
            };

            if handle
                .command_tx
                .send(HttpConnectionCommand::Post {
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
            response,
        } => {
            let Some(handle) = http_connections.get(&connection_id) else {
                let _ = response.send(Err(HttpTransportError::NotFound("unknown ACP connection")));
                return;
            };

            if handle
                .command_tx
                .send(HttpConnectionCommand::AttachListener {
                    session_id,
                    response,
                })
                .is_err()
            {
                http_connections.remove(&connection_id);
            }
        }
        ManagerRequest::HttpDelete {
            connection_id,
            response,
        } => {
            let Some(handle) = http_connections.remove(&connection_id) else {
                let _ = response.send(Err(HttpTransportError::NotFound("unknown ACP connection")));
                return;
            };

            let _ = handle
                .command_tx
                .send(HttpConnectionCommand::Close { response });
        }
    }
}
