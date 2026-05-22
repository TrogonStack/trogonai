use std::convert::Infallible;

use a2a_nats::client::{ClientError, TypedEventStream};
use axum::response::sse::Event;
use futures::Stream;
use futures::StreamExt;

pub fn typed_event_stream_to_sse(
    stream: TypedEventStream,
    jsonrpc_id: serde_json::Value,
) -> impl Stream<Item = Result<Event, Infallible>> {
    stream.map(move |item| {
        let data = match item {
            Ok(response) => {
                let envelope = serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": jsonrpc_id,
                    "result": response,
                });
                serde_json::to_string(&envelope).unwrap_or_else(|e| {
                    serde_json::json!({
                        "jsonrpc": "2.0",
                        "id": null,
                        "error": { "code": -32700, "message": format!("serialize error: {e}") }
                    })
                    .to_string()
                })
            }
            Err(e) => {
                let (code, message) = client_error_to_jsonrpc_code(&e);
                serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": jsonrpc_id,
                    "error": { "code": code, "message": message }
                })
                .to_string()
            }
        };
        Ok(Event::default().data(data))
    })
}

pub fn client_error_to_jsonrpc_code(err: &ClientError) -> (i32, String) {
    match err {
        ClientError::TaskNotFound => (a2a_nats::error::TASK_NOT_FOUND, err.to_string()),
        ClientError::TaskNotCancelable => (a2a_nats::error::TASK_NOT_CANCELABLE, err.to_string()),
        ClientError::PushNotificationNotSupported => {
            (a2a_nats::error::PUSH_NOTIFICATION_NOT_SUPPORTED, err.to_string())
        }
        ClientError::UnsupportedOperation => (a2a_nats::error::UNSUPPORTED_OPERATION, err.to_string()),
        ClientError::ContentTypeNotSupported => (a2a_nats::error::CONTENT_TYPE_NOT_SUPPORTED, err.to_string()),
        ClientError::InvalidAgentResponse => (a2a_nats::error::INVALID_AGENT_RESPONSE, err.to_string()),
        ClientError::AgentUnavailable => (a2a_nats::error::AGENT_UNAVAILABLE, err.to_string()),
        ClientError::JsonRpc { code, message } => (*code, message.clone()),
        _ => (-32603, err.to_string()),
    }
}
