use std::convert::Infallible;
use std::sync::Arc;

use a2a::types::{
    CancelTaskRequest, DeleteTaskPushNotificationConfigRequest, GetTaskPushNotificationConfigRequest, GetTaskRequest,
    ListTaskPushNotificationConfigsRequest, ListTasksRequest, SendMessageRequest, TaskPushNotificationConfig,
};
use a2a_nats::client::{A2aClient, ClientError, ValidatedRpc};
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

use crate::constants::JSONRPC_VERSION;
use crate::sse::{client_error_to_jsonrpc_code, typed_event_stream_to_sse};
use crate::wire::{OutboundError, RestError};

#[derive(Debug, Deserialize)]
pub struct JsonRpcEnvelope {
    pub jsonrpc: Option<String>,
    pub id: Option<Value>,
    pub method: String,
    pub params: Option<Value>,
}

pub async fn handle_jsonrpc<N, J>(
    State(client): State<Arc<A2aClient<N, J>>>,
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

    // JSON-RPC 2.0 requires the version field to be exactly "2.0". Reject
    // anything else with `-32600 Invalid Request` before dispatching, so the
    // bridge doesn't silently front another protocol's calls.
    if envelope.jsonrpc.as_deref() != Some(JSONRPC_VERSION) {
        let body = OutboundError::new(id, -32600, "invalid request: missing or unsupported jsonrpc version");
        return (StatusCode::OK, Json(body)).into_response();
    }

    match envelope.method.as_str() {
        "message/send" => {
            let req: SendMessageRequest = match serde_json::from_value(params) {
                Ok(r) => r,
                Err(e) => return jsonrpc_parse_error(&id, &e.to_string()),
            };
            match client.message_send_validated(&req).await {
                Ok(validated) => jsonrpc_forward(&id, validated),
                Err(e) => jsonrpc_error_response(&id, &e),
            }
        }
        "message/stream" => {
            let req: SendMessageRequest = match serde_json::from_value(params) {
                Ok(r) => r,
                Err(e) => return jsonrpc_parse_error(&id, &e.to_string()),
            };
            match client.message_stream_validated(&req).await {
                Ok((bootstrap, stream)) => {
                    // Mirror tasks/resubscribe and the stdio bridge: emit the
                    // unary response envelope as the opening SSE event so the
                    // caller has a task handle for subsequent JetStream events.
                    let bootstrap_bytes = match bootstrap.body_with_client_id(&id) {
                        Ok(body) => body,
                        Err(e) => return jsonrpc_parse_error(&id, &e.to_string()),
                    };
                    let bootstrap_sse = futures::stream::once(async move {
                        Ok::<Event, Infallible>(Event::default().data(String::from_utf8_lossy(&bootstrap_bytes)))
                    });
                    let sse_stream = typed_event_stream_to_sse(stream, id);
                    sse_response(bootstrap_sse.chain(sse_stream))
                }
                Err(e) => jsonrpc_error_response(&id, &e),
            }
        }
        "tasks/get" => {
            let req: GetTaskRequest = match serde_json::from_value(params) {
                Ok(r) => r,
                Err(e) => return jsonrpc_parse_error(&id, &e.to_string()),
            };
            match client.tasks_get_validated(&req).await {
                Ok(validated) => jsonrpc_forward(&id, validated),
                Err(e) => jsonrpc_error_response(&id, &e),
            }
        }
        "tasks/list" => {
            let req: ListTasksRequest = match serde_json::from_value(params) {
                Ok(r) => r,
                Err(e) => return jsonrpc_parse_error(&id, &e.to_string()),
            };
            match client.tasks_list_validated(&req).await {
                Ok(validated) => jsonrpc_forward(&id, validated),
                Err(e) => jsonrpc_error_response(&id, &e),
            }
        }
        "tasks/cancel" => {
            let req: CancelTaskRequest = match serde_json::from_value(params) {
                Ok(r) => r,
                Err(e) => return jsonrpc_parse_error(&id, &e.to_string()),
            };
            match client.tasks_cancel_validated(&req).await {
                Ok(validated) => jsonrpc_forward(&id, validated),
                Err(e) => jsonrpc_error_response(&id, &e),
            }
        }
        "tasks/resubscribe" => {
            // Accept both shapes:
            //  - a2a-nats-stdio convention: top-level `lastSeq` (camelCase u64)
            //  - older clients: `metadata.lastEventId` (string-encoded u64)
            // top-level `lastSeq` wins so the two binaries stay wire-compatible
            // for the same resume cursor.
            #[derive(Deserialize)]
            #[serde(rename_all = "camelCase")]
            struct ResubscribeParams {
                id: String,
                #[serde(default)]
                last_seq: Option<u64>,
                #[serde(default)]
                metadata: Option<Value>,
            }
            let (task_id_str, last_seq) = match serde_json::from_value::<ResubscribeParams>(params) {
                Ok(p) => {
                    let last_seq = p.last_seq.unwrap_or_else(|| {
                        p.metadata
                            .as_ref()
                            .and_then(|m| m.get("lastEventId"))
                            .and_then(|v| v.as_str())
                            .and_then(|s| s.parse().ok())
                            .unwrap_or(0)
                    });
                    (p.id, last_seq)
                }
                Err(e) => return jsonrpc_parse_error(&id, &e.to_string()),
            };
            let task_id = match A2aTaskId::new(task_id_str) {
                Ok(t) => t,
                Err(e) => return jsonrpc_parse_error(&id, &e.to_string()),
            };
            match client.tasks_resubscribe_validated(&task_id, last_seq).await {
                Ok((snapshot, stream)) => {
                    let snapshot_bytes = match snapshot.body_with_client_id(&id) {
                        Ok(body) => body,
                        Err(e) => return jsonrpc_parse_error(&id, &e.to_string()),
                    };
                    let snapshot_sse = futures::stream::once(async move {
                        Ok::<Event, Infallible>(Event::default().data(String::from_utf8_lossy(&snapshot_bytes)))
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
            match client.push_set_validated(&req).await {
                Ok(validated) => jsonrpc_forward(&id, validated),
                Err(e) => jsonrpc_error_response(&id, &e),
            }
        }
        "tasks/pushNotificationConfig/get" => {
            let req: GetTaskPushNotificationConfigRequest = match serde_json::from_value(params) {
                Ok(r) => r,
                Err(e) => return jsonrpc_parse_error(&id, &e.to_string()),
            };
            match client.push_get_validated(&req).await {
                Ok(validated) => jsonrpc_forward(&id, validated),
                Err(e) => jsonrpc_error_response(&id, &e),
            }
        }
        "tasks/pushNotificationConfig/list" => {
            let req: ListTaskPushNotificationConfigsRequest = match serde_json::from_value(params) {
                Ok(r) => r,
                Err(e) => return jsonrpc_parse_error(&id, &e.to_string()),
            };
            match client.push_list_validated(&req).await {
                Ok(validated) => jsonrpc_forward(&id, validated),
                Err(e) => jsonrpc_error_response(&id, &e),
            }
        }
        "tasks/pushNotificationConfig/delete" => {
            let req: DeleteTaskPushNotificationConfigRequest = match serde_json::from_value(params) {
                Ok(r) => r,
                Err(e) => return jsonrpc_parse_error(&id, &e.to_string()),
            };
            match client.push_delete_validated(&req).await {
                Ok(validated) => jsonrpc_forward(&id, validated),
                Err(e) => jsonrpc_error_response(&id, &e),
            }
        }
        "agent/getAuthenticatedExtendedCard" => match client.agent_card_validated().await {
            Ok(validated) => jsonrpc_forward(&id, validated),
            Err(e) => jsonrpc_error_response(&id, &e),
        },
        method => {
            let body = OutboundError::new(id, -32601, format!("method not found: {method}"));
            (StatusCode::OK, Json(body)).into_response()
        }
    }
}

pub async fn agent_card<N, J>(State(client): State<Arc<A2aClient<N, J>>>) -> Response
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
            // Share the REST status mapping so the well-known card and
            // /v1/card return the same HTTP status + JSON shape for the same
            // ClientError (extended card not configured → 404, invalid agent
            // response → 502, etc.) instead of bucketing everything as 500.
            let (code, message) = client_error_to_jsonrpc_code(&e);
            let status = crate::rest::http_status_for_jsonrpc_code(code);
            (status, Json(RestError::new(code, message))).into_response()
        }
    }
}

fn jsonrpc_forward<T>(id: &Value, validated: ValidatedRpc<T>) -> Response {
    match validated.body_with_client_id(id) {
        Ok(body) => match serde_json::from_slice::<Value>(&body) {
            Ok(value) => (StatusCode::OK, Json(value)).into_response(),
            Err(e) => jsonrpc_parse_error(id, &e.to_string()),
        },
        Err(e) => jsonrpc_parse_error(id, &e.to_string()),
    }
}

fn jsonrpc_error_response(id: &Value, err: &ClientError) -> Response {
    let (code, message) = client_error_to_jsonrpc_code(err);
    let body = OutboundError::new(id.clone(), code, message);
    (StatusCode::OK, Json(body)).into_response()
}

fn jsonrpc_parse_error(id: &Value, message: &str) -> Response {
    let body = OutboundError::new(id.clone(), -32602, format!("invalid params: {message}"));
    (StatusCode::OK, Json(body)).into_response()
}

fn sse_response<S>(stream: S) -> Response
where
    S: Stream<Item = Result<Event, Infallible>> + Send + 'static,
{
    Sse::new(stream).keep_alive(KeepAlive::default()).into_response()
}
