use std::convert::Infallible;
use std::sync::Arc;

use a2a_nats::client::{
    CancelTaskRequest, Client, ClientError, DeleteTaskPushNotificationConfigRequest,
    GetTaskPushNotificationConfigRequest, GetTaskRequest, ListTaskPushNotificationConfigsRequest, ListTasksRequest,
    SendMessageRequest, TaskPushNotificationConfig,
};
use a2a_nats::task_id::A2aTaskId;
use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use futures::{Stream, StreamExt};
use serde::Deserialize;
use serde_json::Value;
use trogon_nats::RequestClient;
use trogon_nats::jetstream::{JetStreamCreateConsumer, JetStreamGetStream, JsAck, JsMessageOf, JsMessageRef};

use crate::sse::{client_error_to_jsonrpc_code, typed_event_stream_to_sse};

#[derive(Debug, Deserialize)]
pub struct JsonRpcEnvelope {
    pub jsonrpc: Option<String>,
    pub id: Option<Value>,
    pub method: String,
    pub params: Option<Value>,
}

pub async fn handle_jsonrpc<N, J>(
    State(client): State<Arc<Client<N, J>>>,
    Json(envelope): Json<JsonRpcEnvelope>,
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
    let id = envelope.id.clone().unwrap_or(Value::Null);
    let params = envelope.params.unwrap_or(Value::Null);

    match envelope.method.as_str() {
        "message/send" => {
            let req: SendMessageRequest = match serde_json::from_value(params) {
                Ok(r) => r,
                Err(e) => return jsonrpc_parse_error(&id, &e.to_string()),
            };
            match client.message_send(&req).await {
                Ok(result) => jsonrpc_ok(&id, result),
                Err(e) => jsonrpc_error_response(&id, &e),
            }
        }
        "message/stream" => {
            let req: SendMessageRequest = match serde_json::from_value(params) {
                Ok(r) => r,
                Err(e) => return jsonrpc_parse_error(&id, &e.to_string()),
            };
            match client.message_stream(&req).await {
                Ok((_bootstrap, stream)) => {
                    let sse_stream = typed_event_stream_to_sse(stream, id);
                    sse_response(sse_stream)
                }
                Err(e) => jsonrpc_error_response(&id, &e),
            }
        }
        "tasks/get" => {
            let req: GetTaskRequest = match serde_json::from_value(params) {
                Ok(r) => r,
                Err(e) => return jsonrpc_parse_error(&id, &e.to_string()),
            };
            match client.tasks_get(&req).await {
                Ok(result) => jsonrpc_ok(&id, result),
                Err(e) => jsonrpc_error_response(&id, &e),
            }
        }
        "tasks/list" => {
            let req: ListTasksRequest = match serde_json::from_value(params) {
                Ok(r) => r,
                Err(e) => return jsonrpc_parse_error(&id, &e.to_string()),
            };
            match client.tasks_list(&req).await {
                Ok(result) => jsonrpc_ok(&id, result),
                Err(e) => jsonrpc_error_response(&id, &e),
            }
        }
        "tasks/cancel" => {
            let req: CancelTaskRequest = match serde_json::from_value(params) {
                Ok(r) => r,
                Err(e) => return jsonrpc_parse_error(&id, &e.to_string()),
            };
            match client.tasks_cancel(&req).await {
                Ok(result) => jsonrpc_ok(&id, result),
                Err(e) => jsonrpc_error_response(&id, &e),
            }
        }
        "tasks/resubscribe" => {
            #[derive(Deserialize)]
            struct ResubscribeParams {
                id: String,
                #[serde(default)]
                metadata: Option<Value>,
            }
            let _meta: Option<Value>;
            let (task_id_str, last_seq) = match serde_json::from_value::<ResubscribeParams>(params) {
                Ok(p) => {
                    _meta = p.metadata;
                    let last_seq: u64 = _meta
                        .as_ref()
                        .and_then(|m| m.get("lastEventId"))
                        .and_then(|v| v.as_str())
                        .and_then(|s| s.parse().ok())
                        .unwrap_or(0);
                    (p.id, last_seq)
                }
                Err(e) => return jsonrpc_parse_error(&id, &e.to_string()),
            };
            let task_id = match A2aTaskId::new(task_id_str) {
                Ok(t) => t,
                Err(e) => return jsonrpc_parse_error(&id, &e.to_string()),
            };
            match client.tasks_resubscribe(&task_id, last_seq).await {
                Ok((snapshot, stream)) => {
                    let snapshot_event = serde_json::json!({
                        "jsonrpc": "2.0",
                        "id": id.clone(),
                        "result": snapshot,
                    });
                    let snapshot_sse = futures::stream::once(async move {
                        Ok::<Event, Infallible>(
                            Event::default().data(serde_json::to_string(&snapshot_event).unwrap_or_default()),
                        )
                    });
                    let sse_stream = typed_event_stream_to_sse(stream, id);
                    sse_response(snapshot_sse.chain(sse_stream))
                }
                Err(e) => jsonrpc_error_response(&id, &e),
            }
        }
        "tasks/pushNotificationConfig/set" => {
            let req: TaskPushNotificationConfig = match serde_json::from_value(params) {
                Ok(r) => r,
                Err(e) => return jsonrpc_parse_error(&id, &e.to_string()),
            };
            match client.push_set(&req).await {
                Ok(result) => jsonrpc_ok(&id, result),
                Err(e) => jsonrpc_error_response(&id, &e),
            }
        }
        "tasks/pushNotificationConfig/get" => {
            let req: GetTaskPushNotificationConfigRequest = match serde_json::from_value(params) {
                Ok(r) => r,
                Err(e) => return jsonrpc_parse_error(&id, &e.to_string()),
            };
            match client.push_get(&req).await {
                Ok(result) => jsonrpc_ok(&id, result),
                Err(e) => jsonrpc_error_response(&id, &e),
            }
        }
        "tasks/pushNotificationConfig/list" => {
            let req: ListTaskPushNotificationConfigsRequest = match serde_json::from_value(params) {
                Ok(r) => r,
                Err(e) => return jsonrpc_parse_error(&id, &e.to_string()),
            };
            match client.push_list(&req).await {
                Ok(result) => jsonrpc_ok(&id, result),
                Err(e) => jsonrpc_error_response(&id, &e),
            }
        }
        "tasks/pushNotificationConfig/delete" => {
            let req: DeleteTaskPushNotificationConfigRequest = match serde_json::from_value(params) {
                Ok(r) => r,
                Err(e) => return jsonrpc_parse_error(&id, &e.to_string()),
            };
            match client.push_delete(&req).await {
                Ok(()) => jsonrpc_ok(&id, Value::Null),
                Err(e) => jsonrpc_error_response(&id, &e),
            }
        }
        "agent/getAuthenticatedExtendedCard" => match client.agent_card().await {
            Ok(result) => jsonrpc_ok(&id, result),
            Err(e) => jsonrpc_error_response(&id, &e),
        },
        method => {
            let body = serde_json::json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": { "code": -32601, "message": format!("method not found: {method}") }
            });
            (StatusCode::OK, Json(body)).into_response()
        }
    }
}

pub async fn agent_card<N, J>(State(client): State<Arc<Client<N, J>>>) -> Response
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
        Ok(card) => Json(card).into_response(),
        Err(e) => {
            let (code, message) = client_error_to_jsonrpc_code(&e);
            let status = if code == a2a_nats::error::AGENT_UNAVAILABLE {
                StatusCode::SERVICE_UNAVAILABLE
            } else {
                StatusCode::INTERNAL_SERVER_ERROR
            };
            (status, message).into_response()
        }
    }
}

fn jsonrpc_ok<T: serde::Serialize>(id: &Value, result: T) -> Response {
    let body = serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": result,
    });
    (StatusCode::OK, Json(body)).into_response()
}

fn jsonrpc_error_response(id: &Value, err: &ClientError) -> Response {
    let (code, message) = client_error_to_jsonrpc_code(err);
    let body = serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": { "code": code, "message": message }
    });
    (StatusCode::OK, Json(body)).into_response()
}

fn jsonrpc_parse_error(id: &Value, message: &str) -> Response {
    let body = serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": { "code": -32602, "message": format!("invalid params: {message}") }
    });
    (StatusCode::OK, Json(body)).into_response()
}

fn sse_response<S>(stream: S) -> Response
where
    S: Stream<Item = Result<Event, Infallible>> + Send + 'static,
{
    Sse::new(stream).keep_alive(KeepAlive::default()).into_response()
}
