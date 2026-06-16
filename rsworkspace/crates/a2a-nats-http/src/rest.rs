//! HTTP+JSON/REST binding for the A2A protocol surface.
//!
//! These routes are functional equivalents of the JSON-RPC endpoints in
//! [`crate::handlers`]; they exist so clients that prefer REST verbs over
//! JSON-RPC framing can talk to the same agent runtime without needing a
//! JSON-RPC envelope. Each handler builds the same `a2a-types` request
//! struct the JSON-RPC handler does and delegates to the same
//! `a2a_nats::client::A2aClient` method.

use std::convert::Infallible;
use std::sync::Arc;

use a2a_nats::client::{
    CancelTaskRequest, A2aClient, ClientError, DeleteTaskPushNotificationConfigRequest,
    GetTaskPushNotificationConfigRequest, GetTaskRequest, ListTaskPushNotificationConfigsRequest, ListTasksRequest,
    SendMessageRequest, TaskPushNotificationConfig,
};
use a2a_nats::task_id::A2aTaskId;
use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use futures::{Stream, StreamExt};
use serde::Deserialize;
use serde_json::Value;
use trogon_nats::RequestClient;
use trogon_nats::jetstream::{JetStreamCreateConsumer, JetStreamGetStream, JsAck, JsMessageOf, JsMessageRef};

use crate::sse::{client_error_to_jsonrpc_code, typed_event_stream_to_sse};

/// Build a `Router` of REST routes that can be merged into the top-level router.
pub fn router<N, J>() -> axum::Router<Arc<A2aClient<N, J>>>
where
    N: RequestClient + Clone + Send + Sync + 'static,
    J: JetStreamGetStream + Clone + Send + Sync + 'static,
    JsMessageOf<J>: JsMessageRef + JsAck<Error: std::fmt::Display + Send + 'static> + Send + 'static,
    <J as JetStreamGetStream>::Stream: Send + 'static,
    <<J as JetStreamGetStream>::Stream as JetStreamCreateConsumer>::Consumer: Send + 'static,
    <<<J as JetStreamGetStream>::Stream as JetStreamCreateConsumer>::Consumer as trogon_nats::jetstream::JetStreamConsumer>::Messages: Send + 'static,
    <<<J as JetStreamGetStream>::Stream as JetStreamCreateConsumer>::Consumer as trogon_nats::jetstream::JetStreamConsumer>::MessagesError: std::fmt::Display + Send + 'static,
    <<<J as JetStreamGetStream>::Stream as JetStreamCreateConsumer>::Consumer as trogon_nats::jetstream::JetStreamConsumer>::StreamError: std::fmt::Display + Send + 'static,
{
    axum::Router::new()
        .route("/v1/card", get(get_card::<N, J>))
        .route("/v1/message:send", post(post_message_send::<N, J>))
        .route("/v1/message:stream", post(post_message_stream::<N, J>))
        .route("/v1/tasks", get(get_tasks_list::<N, J>))
        .route("/v1/tasks/{id}", get(get_task::<N, J>))
        .route("/v1/tasks/{id}/cancel", post(post_task_cancel::<N, J>))
        .route("/v1/tasks/{id}/subscribe", get(get_task_subscribe::<N, J>))
        .route(
            "/v1/tasks/{id}/pushNotificationConfigs",
            post(post_push_set::<N, J>).get(get_push_list::<N, J>),
        )
        .route(
            "/v1/tasks/{id}/pushNotificationConfigs/{configId}",
            get(get_push_config::<N, J>).delete(delete_push_config::<N, J>),
        )
        .route("/v1/tasks/{id}/pushNotificationConfigs/{configId}/_keep_delete_chain", delete(noop))
}

async fn noop() -> Response {
    StatusCode::NO_CONTENT.into_response()
}

async fn get_card<N, J>(State(client): State<Arc<A2aClient<N, J>>>) -> Response
where
    N: RequestClient + Clone + Send + Sync + 'static,
    J: JetStreamGetStream + Clone + Send + Sync + 'static,
    JsMessageOf<J>: JsMessageRef + JsAck<Error: std::fmt::Display + Send + 'static> + Send + 'static,
    <J as JetStreamGetStream>::Stream: Send + 'static,
    <<J as JetStreamGetStream>::Stream as JetStreamCreateConsumer>::Consumer: Send + 'static,
    <<<J as JetStreamGetStream>::Stream as JetStreamCreateConsumer>::Consumer as trogon_nats::jetstream::JetStreamConsumer>::Messages: Send + 'static,
    <<<J as JetStreamGetStream>::Stream as JetStreamCreateConsumer>::Consumer as trogon_nats::jetstream::JetStreamConsumer>::MessagesError: std::fmt::Display + Send + 'static,
    <<<J as JetStreamGetStream>::Stream as JetStreamCreateConsumer>::Consumer as trogon_nats::jetstream::JetStreamConsumer>::StreamError: std::fmt::Display + Send + 'static,
{
    match client.agent_card().await {
        Ok(card) => (StatusCode::OK, Json(card)).into_response(),
        Err(e) => rest_error_response(&e),
    }
}

async fn post_message_send<N, J>(State(client): State<Arc<A2aClient<N, J>>>, Json(req): Json<SendMessageRequest>) -> Response
where
    N: RequestClient + Clone + Send + Sync + 'static,
    J: JetStreamGetStream + Clone + Send + Sync + 'static,
    JsMessageOf<J>: JsMessageRef + JsAck<Error: std::fmt::Display + Send + 'static> + Send + 'static,
    <J as JetStreamGetStream>::Stream: Send + 'static,
    <<J as JetStreamGetStream>::Stream as JetStreamCreateConsumer>::Consumer: Send + 'static,
    <<<J as JetStreamGetStream>::Stream as JetStreamCreateConsumer>::Consumer as trogon_nats::jetstream::JetStreamConsumer>::Messages: Send + 'static,
    <<<J as JetStreamGetStream>::Stream as JetStreamCreateConsumer>::Consumer as trogon_nats::jetstream::JetStreamConsumer>::MessagesError: std::fmt::Display + Send + 'static,
    <<<J as JetStreamGetStream>::Stream as JetStreamCreateConsumer>::Consumer as trogon_nats::jetstream::JetStreamConsumer>::StreamError: std::fmt::Display + Send + 'static,
{
    match client.message_send(&req).await {
        Ok(result) => (StatusCode::OK, Json(result)).into_response(),
        Err(e) => rest_error_response(&e),
    }
}

async fn post_message_stream<N, J>(
    State(client): State<Arc<A2aClient<N, J>>>,
    Json(req): Json<SendMessageRequest>,
) -> Response
where
    N: RequestClient + Clone + Send + Sync + 'static,
    J: JetStreamGetStream + Clone + Send + Sync + 'static,
    JsMessageOf<J>: JsMessageRef + JsAck<Error: std::fmt::Display + Send + 'static> + Send + 'static,
    <J as JetStreamGetStream>::Stream: Send + 'static,
    <<J as JetStreamGetStream>::Stream as JetStreamCreateConsumer>::Consumer: Send + 'static,
    <<<J as JetStreamGetStream>::Stream as JetStreamCreateConsumer>::Consumer as trogon_nats::jetstream::JetStreamConsumer>::Messages: Send + 'static,
    <<<J as JetStreamGetStream>::Stream as JetStreamCreateConsumer>::Consumer as trogon_nats::jetstream::JetStreamConsumer>::MessagesError: std::fmt::Display + Send + 'static,
    <<<J as JetStreamGetStream>::Stream as JetStreamCreateConsumer>::Consumer as trogon_nats::jetstream::JetStreamConsumer>::StreamError: std::fmt::Display + Send + 'static,
{
    match client.message_stream(&req).await {
        Ok((_bootstrap, stream)) => sse_response(typed_event_stream_to_sse(stream, Value::Null)),
        Err(e) => rest_error_response(&e),
    }
}

#[derive(Debug, Deserialize, Default)]
struct ListTasksQuery {
    #[serde(default)]
    tenant: Option<String>,
    #[serde(default)]
    page_size: Option<i32>,
    #[serde(default)]
    page_token: Option<String>,
}

async fn get_tasks_list<N, J>(
    State(client): State<Arc<A2aClient<N, J>>>,
    Query(q): Query<ListTasksQuery>,
) -> Response
where
    N: RequestClient + Clone + Send + Sync + 'static,
    J: JetStreamGetStream + Clone + Send + Sync + 'static,
    JsMessageOf<J>: JsMessageRef + JsAck<Error: std::fmt::Display + Send + 'static> + Send + 'static,
    <J as JetStreamGetStream>::Stream: Send + 'static,
    <<J as JetStreamGetStream>::Stream as JetStreamCreateConsumer>::Consumer: Send + 'static,
    <<<J as JetStreamGetStream>::Stream as JetStreamCreateConsumer>::Consumer as trogon_nats::jetstream::JetStreamConsumer>::Messages: Send + 'static,
    <<<J as JetStreamGetStream>::Stream as JetStreamCreateConsumer>::Consumer as trogon_nats::jetstream::JetStreamConsumer>::MessagesError: std::fmt::Display + Send + 'static,
    <<<J as JetStreamGetStream>::Stream as JetStreamCreateConsumer>::Consumer as trogon_nats::jetstream::JetStreamConsumer>::StreamError: std::fmt::Display + Send + 'static,
{
    let req = ListTasksRequest {
        tenant: q.tenant,
        context_id: None,
        status: None,
        page_size: q.page_size,
        page_token: q.page_token,
        history_length: None,
        status_timestamp_after: None,
        include_artifacts: None,
    };
    match client.tasks_list(&req).await {
        Ok(result) => (StatusCode::OK, Json(result)).into_response(),
        Err(e) => rest_error_response(&e),
    }
}

#[derive(Debug, Deserialize, Default)]
struct GetTaskQuery {
    #[serde(default)]
    tenant: Option<String>,
    #[serde(default, alias = "historyLength")]
    history_length: Option<i32>,
}

async fn get_task<N, J>(
    State(client): State<Arc<A2aClient<N, J>>>,
    Path(id): Path<String>,
    Query(q): Query<GetTaskQuery>,
) -> Response
where
    N: RequestClient + Clone + Send + Sync + 'static,
    J: JetStreamGetStream + Clone + Send + Sync + 'static,
    JsMessageOf<J>: JsMessageRef + JsAck<Error: std::fmt::Display + Send + 'static> + Send + 'static,
    <J as JetStreamGetStream>::Stream: Send + 'static,
    <<J as JetStreamGetStream>::Stream as JetStreamCreateConsumer>::Consumer: Send + 'static,
    <<<J as JetStreamGetStream>::Stream as JetStreamCreateConsumer>::Consumer as trogon_nats::jetstream::JetStreamConsumer>::Messages: Send + 'static,
    <<<J as JetStreamGetStream>::Stream as JetStreamCreateConsumer>::Consumer as trogon_nats::jetstream::JetStreamConsumer>::MessagesError: std::fmt::Display + Send + 'static,
    <<<J as JetStreamGetStream>::Stream as JetStreamCreateConsumer>::Consumer as trogon_nats::jetstream::JetStreamConsumer>::StreamError: std::fmt::Display + Send + 'static,
{
    let req = GetTaskRequest {
        id,
        tenant: q.tenant,
        history_length: q.history_length,
    };
    match client.tasks_get(&req).await {
        Ok(result) => (StatusCode::OK, Json(result)).into_response(),
        Err(e) => rest_error_response(&e),
    }
}

#[derive(Debug, Deserialize, Default)]
struct CancelQuery {
    #[serde(default)]
    tenant: Option<String>,
}

async fn post_task_cancel<N, J>(
    State(client): State<Arc<A2aClient<N, J>>>,
    Path(id): Path<String>,
    Query(q): Query<CancelQuery>,
) -> Response
where
    N: RequestClient + Clone + Send + Sync + 'static,
    J: JetStreamGetStream + Clone + Send + Sync + 'static,
    JsMessageOf<J>: JsMessageRef + JsAck<Error: std::fmt::Display + Send + 'static> + Send + 'static,
    <J as JetStreamGetStream>::Stream: Send + 'static,
    <<J as JetStreamGetStream>::Stream as JetStreamCreateConsumer>::Consumer: Send + 'static,
    <<<J as JetStreamGetStream>::Stream as JetStreamCreateConsumer>::Consumer as trogon_nats::jetstream::JetStreamConsumer>::Messages: Send + 'static,
    <<<J as JetStreamGetStream>::Stream as JetStreamCreateConsumer>::Consumer as trogon_nats::jetstream::JetStreamConsumer>::MessagesError: std::fmt::Display + Send + 'static,
    <<<J as JetStreamGetStream>::Stream as JetStreamCreateConsumer>::Consumer as trogon_nats::jetstream::JetStreamConsumer>::StreamError: std::fmt::Display + Send + 'static,
{
    let req = CancelTaskRequest {
        id,
        tenant: q.tenant,
        metadata: None,
    };
    match client.tasks_cancel(&req).await {
        Ok(result) => (StatusCode::OK, Json(result)).into_response(),
        Err(e) => rest_error_response(&e),
    }
}

#[derive(Debug, Deserialize, Default)]
struct SubscribeQuery {
    #[serde(default, alias = "lastEventId")]
    last_event_id: Option<u64>,
}

async fn get_task_subscribe<N, J>(
    State(client): State<Arc<A2aClient<N, J>>>,
    Path(id): Path<String>,
    Query(q): Query<SubscribeQuery>,
) -> Response
where
    N: RequestClient + Clone + Send + Sync + 'static,
    J: JetStreamGetStream + Clone + Send + Sync + 'static,
    JsMessageOf<J>: JsMessageRef + JsAck<Error: std::fmt::Display + Send + 'static> + Send + 'static,
    <J as JetStreamGetStream>::Stream: Send + 'static,
    <<J as JetStreamGetStream>::Stream as JetStreamCreateConsumer>::Consumer: Send + 'static,
    <<<J as JetStreamGetStream>::Stream as JetStreamCreateConsumer>::Consumer as trogon_nats::jetstream::JetStreamConsumer>::Messages: Send + 'static,
    <<<J as JetStreamGetStream>::Stream as JetStreamCreateConsumer>::Consumer as trogon_nats::jetstream::JetStreamConsumer>::MessagesError: std::fmt::Display + Send + 'static,
    <<<J as JetStreamGetStream>::Stream as JetStreamCreateConsumer>::Consumer as trogon_nats::jetstream::JetStreamConsumer>::StreamError: std::fmt::Display + Send + 'static,
{
    let task_id = match A2aTaskId::new(id) {
        Ok(t) => t,
        Err(e) => return (StatusCode::BAD_REQUEST, e.to_string()).into_response(),
    };
    match client.tasks_resubscribe(&task_id, q.last_event_id.unwrap_or(0)).await {
        Ok((snapshot, stream)) => {
            let snapshot_event = serde_json::json!({ "result": snapshot });
            let snapshot_sse = futures::stream::once(async move {
                Ok::<Event, Infallible>(
                    Event::default().data(serde_json::to_string(&snapshot_event).unwrap_or_default()),
                )
            });
            sse_response(snapshot_sse.chain(typed_event_stream_to_sse(stream, Value::Null)))
        }
        Err(e) => rest_error_response(&e),
    }
}

async fn post_push_set<N, J>(
    State(client): State<Arc<A2aClient<N, J>>>,
    Path(_id): Path<String>,
    Json(req): Json<TaskPushNotificationConfig>,
) -> Response
where
    N: RequestClient + Clone + Send + Sync + 'static,
    J: JetStreamGetStream + Clone + Send + Sync + 'static,
    JsMessageOf<J>: JsMessageRef + JsAck<Error: std::fmt::Display + Send + 'static> + Send + 'static,
    <J as JetStreamGetStream>::Stream: Send + 'static,
    <<J as JetStreamGetStream>::Stream as JetStreamCreateConsumer>::Consumer: Send + 'static,
    <<<J as JetStreamGetStream>::Stream as JetStreamCreateConsumer>::Consumer as trogon_nats::jetstream::JetStreamConsumer>::Messages: Send + 'static,
    <<<J as JetStreamGetStream>::Stream as JetStreamCreateConsumer>::Consumer as trogon_nats::jetstream::JetStreamConsumer>::MessagesError: std::fmt::Display + Send + 'static,
    <<<J as JetStreamGetStream>::Stream as JetStreamCreateConsumer>::Consumer as trogon_nats::jetstream::JetStreamConsumer>::StreamError: std::fmt::Display + Send + 'static,
{
    match client.push_set(&req).await {
        Ok(result) => (StatusCode::OK, Json(result)).into_response(),
        Err(e) => rest_error_response(&e),
    }
}

#[derive(Debug, Deserialize, Default)]
struct PushListQuery {
    #[serde(default)]
    tenant: Option<String>,
}

async fn get_push_list<N, J>(
    State(client): State<Arc<A2aClient<N, J>>>,
    Path(id): Path<String>,
    Query(q): Query<PushListQuery>,
) -> Response
where
    N: RequestClient + Clone + Send + Sync + 'static,
    J: JetStreamGetStream + Clone + Send + Sync + 'static,
    JsMessageOf<J>: JsMessageRef + JsAck<Error: std::fmt::Display + Send + 'static> + Send + 'static,
    <J as JetStreamGetStream>::Stream: Send + 'static,
    <<J as JetStreamGetStream>::Stream as JetStreamCreateConsumer>::Consumer: Send + 'static,
    <<<J as JetStreamGetStream>::Stream as JetStreamCreateConsumer>::Consumer as trogon_nats::jetstream::JetStreamConsumer>::Messages: Send + 'static,
    <<<J as JetStreamGetStream>::Stream as JetStreamCreateConsumer>::Consumer as trogon_nats::jetstream::JetStreamConsumer>::MessagesError: std::fmt::Display + Send + 'static,
    <<<J as JetStreamGetStream>::Stream as JetStreamCreateConsumer>::Consumer as trogon_nats::jetstream::JetStreamConsumer>::StreamError: std::fmt::Display + Send + 'static,
{
    let req = ListTaskPushNotificationConfigsRequest {
        task_id: id,
        tenant: q.tenant,
        page_size: None,
        page_token: None,
    };
    match client.push_list(&req).await {
        Ok(result) => (StatusCode::OK, Json(result)).into_response(),
        Err(e) => rest_error_response(&e),
    }
}

async fn get_push_config<N, J>(
    State(client): State<Arc<A2aClient<N, J>>>,
    Path((id, config_id)): Path<(String, String)>,
) -> Response
where
    N: RequestClient + Clone + Send + Sync + 'static,
    J: JetStreamGetStream + Clone + Send + Sync + 'static,
    JsMessageOf<J>: JsMessageRef + JsAck<Error: std::fmt::Display + Send + 'static> + Send + 'static,
    <J as JetStreamGetStream>::Stream: Send + 'static,
    <<J as JetStreamGetStream>::Stream as JetStreamCreateConsumer>::Consumer: Send + 'static,
    <<<J as JetStreamGetStream>::Stream as JetStreamCreateConsumer>::Consumer as trogon_nats::jetstream::JetStreamConsumer>::Messages: Send + 'static,
    <<<J as JetStreamGetStream>::Stream as JetStreamCreateConsumer>::Consumer as trogon_nats::jetstream::JetStreamConsumer>::MessagesError: std::fmt::Display + Send + 'static,
    <<<J as JetStreamGetStream>::Stream as JetStreamCreateConsumer>::Consumer as trogon_nats::jetstream::JetStreamConsumer>::StreamError: std::fmt::Display + Send + 'static,
{
    let req = GetTaskPushNotificationConfigRequest {
        task_id: id,
        id: config_id,
        tenant: None,
    };
    match client.push_get(&req).await {
        Ok(result) => (StatusCode::OK, Json(result)).into_response(),
        Err(e) => rest_error_response(&e),
    }
}

async fn delete_push_config<N, J>(
    State(client): State<Arc<A2aClient<N, J>>>,
    Path((id, config_id)): Path<(String, String)>,
) -> Response
where
    N: RequestClient + Clone + Send + Sync + 'static,
    J: JetStreamGetStream + Clone + Send + Sync + 'static,
    JsMessageOf<J>: JsMessageRef + JsAck<Error: std::fmt::Display + Send + 'static> + Send + 'static,
    <J as JetStreamGetStream>::Stream: Send + 'static,
    <<J as JetStreamGetStream>::Stream as JetStreamCreateConsumer>::Consumer: Send + 'static,
    <<<J as JetStreamGetStream>::Stream as JetStreamCreateConsumer>::Consumer as trogon_nats::jetstream::JetStreamConsumer>::Messages: Send + 'static,
    <<<J as JetStreamGetStream>::Stream as JetStreamCreateConsumer>::Consumer as trogon_nats::jetstream::JetStreamConsumer>::MessagesError: std::fmt::Display + Send + 'static,
    <<<J as JetStreamGetStream>::Stream as JetStreamCreateConsumer>::Consumer as trogon_nats::jetstream::JetStreamConsumer>::StreamError: std::fmt::Display + Send + 'static,
{
    let req = DeleteTaskPushNotificationConfigRequest {
        task_id: id,
        id: config_id,
        tenant: None,
    };
    match client.push_delete(&req).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => rest_error_response(&e),
    }
}

fn rest_error_response(err: &ClientError) -> Response {
    let (code, message) = client_error_to_jsonrpc_code(err);
    let status = http_status_for_jsonrpc_code(code);
    let body = serde_json::json!({
        "error": { "code": code, "message": message },
    });
    (status, Json(body)).into_response()
}

fn http_status_for_jsonrpc_code(code: i32) -> StatusCode {
    use a2a_nats::error::*;
    match code {
        TASK_NOT_FOUND => StatusCode::NOT_FOUND,
        TASK_NOT_CANCELABLE => StatusCode::CONFLICT,
        PUSH_NOTIFICATION_NOT_SUPPORTED | UNSUPPORTED_OPERATION => StatusCode::NOT_IMPLEMENTED,
        CONTENT_TYPE_NOT_SUPPORTED => StatusCode::UNSUPPORTED_MEDIA_TYPE,
        EXTENDED_AGENT_CARD_NOT_CONFIGURED => StatusCode::NOT_FOUND,
        EXTENSION_SUPPORT_REQUIRED | VERSION_NOT_SUPPORTED => StatusCode::BAD_REQUEST,
        AGENT_UNAVAILABLE => StatusCode::SERVICE_UNAVAILABLE,
        INVALID_AGENT_RESPONSE => StatusCode::BAD_GATEWAY,
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

fn sse_response<S>(stream: S) -> Response
where
    S: Stream<Item = Result<Event, Infallible>> + Send + 'static,
{
    Sse::new(stream).keep_alive(KeepAlive::default()).into_response()
}
