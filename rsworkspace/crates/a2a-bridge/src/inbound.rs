//! HTTPS JSON-RPC ingress: terminate auth, remap to **`a2a.gateway.{agent_id}.{method}`** (`/` → `.`).

use std::convert::Infallible;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};

use async_trait::async_trait;
use axum::{
    Router,
    body::Bytes,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{
        IntoResponse, Response,
        sse::{Event, KeepAlive, Sse},
    },
    routing::post,
};
use futures_util::StreamExt;
use futures_util::stream::{self, BoxStream};
use serde_json::Value;
use serde_json::json;

use crate::auth::AuthCalloutClient;
use crate::error::BridgeError;
use crate::identity::{BridgeUserJwt, CallerHttpsAuth};

const AGENT_ID_HEADER: &str = "x-a2a-agent-id";

/// Tracks [`A2A_PLAN.md`] subject dotted method mapping (e.g. `message/send` → `message.send`).
pub fn gateway_method_to_subject_dots(method: &str) -> String {
    method.replace('/', ".")
}

/// Publication target **`a2a.gateway.{agent_id}.{dotted_method}`**.
pub fn build_gateway_subject(agent_id: &str, method: &str) -> String {
    format!("a2a.gateway.{}.{}", agent_id, gateway_method_to_subject_dots(method))
}

/// Subject filter scaffolding for SSE ↔ JetStream mapping (`events.>` tail per consumer setup).
#[must_use]
pub fn task_events_wild_subject(task_id: &str) -> String {
    format!("a2a.task.{task_id}.events.>")
}

fn is_sse_jsonrpc_method(method: &str) -> bool {
    matches!(method, "message/stream" | "tasks/resubscribe")
}

#[derive(Clone)]
pub struct AppState {
    pub(crate) auth: Arc<dyn AuthCalloutClient>,
    pub(crate) publisher: Arc<dyn InboundGatewayPublish>,
}

impl AppState {
    pub fn new(auth: Arc<dyn AuthCalloutClient>, publisher: Arc<dyn InboundGatewayPublish>) -> Self {
        Self { auth, publisher }
    }
}

#[async_trait]
pub trait InboundGatewayPublish: Send + Sync {
    async fn publish_unary_to_gateway(
        &self,
        subject: &str,
        caller_jwt: &BridgeUserJwt,
        jsonrpc_payload: &[u8],
    ) -> Result<Bytes, BridgeError>;
}

#[derive(Clone, Copy, Debug, Default)]
pub struct StubInboundGatewayPublish;

#[async_trait]
impl InboundGatewayPublish for StubInboundGatewayPublish {
    async fn publish_unary_to_gateway(
        &self,
        _subject: &str,
        _caller_jwt: &BridgeUserJwt,
        _jsonrpc_payload: &[u8],
    ) -> Result<Bytes, BridgeError> {
        unimplemented!("wired when a2a-nats publish surface is finalized")
    }
}

#[derive(Clone)]
pub struct RecordingInboundPublisher {
    pub last_subject: Arc<Mutex<Option<String>>>,
}

impl RecordingInboundPublisher {
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

#[async_trait]
impl InboundGatewayPublish for RecordingInboundPublisher {
    async fn publish_unary_to_gateway(
        &self,
        subject: &str,
        _caller_jwt: &BridgeUserJwt,
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

fn caller_auth_from(headers: &HeaderMap) -> Result<CallerHttpsAuth, BridgeError> {
    let raw = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .ok_or(BridgeError::MissingAuthorization)?;
    Ok(CallerHttpsAuth::new(raw.to_owned()))
}

fn agent_id_from(headers: &HeaderMap) -> Result<String, BridgeError> {
    headers
        .get(AGENT_ID_HEADER)
        .and_then(|v| v.to_str().ok())
        .map(std::borrow::ToOwned::to_owned)
        .ok_or(BridgeError::MissingAgentHeader)
}

pub fn gateway_router(state: AppState) -> Router {
    Router::new().route("/", post(a2a_post)).with_state(state)
}

async fn a2a_post(State(state): State<AppState>, headers: HeaderMap, body: Bytes) -> Response {
    match handle_jsonrpc(headers, body, &state).await {
        Ok(r) => r,
        Err(e) => bridge_error_into_response(e),
    }
}

fn bridge_error_into_response(e: BridgeError) -> Response {
    let status = StatusCode::BAD_GATEWAY;
    let body = json!({"error": e.to_string()});
    (status, axum::Json(body)).into_response()
}

pub(crate) async fn handle_jsonrpc(headers: HeaderMap, body: Bytes, state: &AppState) -> Result<Response, BridgeError> {
    let caller_auth = caller_auth_from(&headers)?;
    let agent_id = agent_id_from(&headers)?;
    let user_jwt = state.auth.mint(&caller_auth).await?;

    let v: Value = serde_json::from_slice(&body).map_err(BridgeError::Deserialize)?;
    let Some(method) = v.get("method").and_then(Value::as_str) else {
        return Err(BridgeError::MissingJsonRpcMethod);
    };

    let subject = build_gateway_subject(&agent_id, method);

    if is_sse_jsonrpc_method(method) {
        let wild = sse_task_consumer_filter_skeleton(&v);
        return Ok(sse_skeleton_response(method.to_owned(), subject.clone(), wild).into_response());
    }

    let out = state
        .publisher
        .publish_unary_to_gateway(&subject, &user_jwt, body.as_ref())
        .await?;
    Ok(Response::builder()
        .status(StatusCode::OK)
        .header(axum::http::header::CONTENT_TYPE, "application/json")
        .body(axum::body::Body::from(out))
        .map_err(|e| BridgeError::NatsPublish(e.to_string()))?)
}

fn sse_task_consumer_filter_skeleton(body: &Value) -> Option<String> {
    let tid = body
        .pointer("/params/message/taskId")
        .or_else(|| body.pointer("/params/taskId"))
        .and_then(Value::as_str)?;
    Some(task_events_wild_subject(tid))
}

fn sse_skeleton_body(
    method: String,
    _gateway_publish_subject: String,
    consumer_wild_hint: Option<String>,
) -> BoxStream<'static, Result<Event, Infallible>> {
    let consumer_wild = consumer_wild_hint.unwrap_or_else(|| "a2a.task.<task_id>.events.>".into());
    stream::poll_fn(
        move |_cx: &mut Context<'_>| -> Poll<Option<Result<Event, Infallible>>> {
            let _capture = (&method, consumer_wild.as_str());
            unimplemented!(
                "wired when a2a-nats publish surface is finalized for SSE JetStream framing; JetStream wildcard: {}",
                consumer_wild
            )
        },
    )
    .boxed()
}

fn sse_skeleton_response(method: String, gateway_subject: String, wild: Option<String>) -> Response {
    Sse::new(sse_skeleton_body(method, gateway_subject, wild))
        .keep_alive(KeepAlive::default())
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::{HeaderMap, header::AUTHORIZATION};

    #[derive(Clone)]
    struct FakeMint;

    #[async_trait]
    impl AuthCalloutClient for FakeMint {
        async fn mint(&self, _caller_auth: &CallerHttpsAuth) -> Result<BridgeUserJwt, BridgeError> {
            Ok(BridgeUserJwt::new("known-test-jwt"))
        }
    }

    fn test_headers(agent: &str) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert(AUTHORIZATION, "Bearer https-test".parse().unwrap());
        h.insert(
            axum::http::HeaderName::from_static(AGENT_ID_HEADER),
            agent.parse().unwrap(),
        );
        h
    }

    #[tokio::test]
    async fn unary_resolves_gateway_subject_via_publisher_capture() {
        let pubber = RecordingInboundPublisher::new();
        let state = AppState::new(Arc::new(FakeMint), Arc::new(pubber.clone()));

        let body = Bytes::copy_from_slice(br#"{"jsonrpc":"2.0","method":"message/send","id":1,"params":{}}"#);
        let _ = handle_jsonrpc(test_headers("support-bot"), body, &state).await.unwrap();

        assert_eq!(
            pubber.peek_subject().as_deref(),
            Some("a2a.gateway.support-bot.message.send")
        );
    }

    #[test]
    fn gateway_subject_dots_method_segments() {
        assert_eq!(
            build_gateway_subject("a", "tasks/pushNotificationConfig/set"),
            "a2a.gateway.a.tasks.pushNotificationConfig.set"
        );
    }
}
