use std::convert::Infallible;

use a2a_nats::client::ClientError;
use a2a_nats::client::event_stream::TypedEventStream;
use axum::response::sse::Event;
use futures::Stream;
use futures::StreamExt;
use jsonrpc_nats::{INTERNAL_ERROR, ResponseId};

use crate::wire;

pub fn typed_event_stream_to_sse(
    stream: TypedEventStream,
    jsonrpc_id: ResponseId,
) -> impl Stream<Item = Result<Event, Infallible>> {
    stream.map(move |item| {
        let envelope = match item {
            // The A2A spec streams every chunk as a JSON-RPC success response
            // repeating the request id, terminated by the one whose result is
            // final. The id is the caller's, not the transport's, because the
            // caller correlates against the id they sent.
            Ok(response) => wire::success(&jsonrpc_id, &response).unwrap_or_else(|e| {
                // Server-side serialization failure is `-32603` Internal, not
                // `-32700` Parse error (that's reserved for invalid JSON on the
                // way in). Echo the original id so the client can correlate the
                // failure with their stream subscription.
                wire::error(&jsonrpc_id, INTERNAL_ERROR, format!("serialize error: {e}"))
            }),
            Err(e) => {
                let (code, message) = client_error_to_jsonrpc_code(&e);
                wire::error(&jsonrpc_id, code, message)
            }
        };
        Ok(Event::default().data(envelope.to_string()))
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
        ClientError::ExtendedAgentCardNotConfigured => {
            (a2a_nats::error::EXTENDED_AGENT_CARD_NOT_CONFIGURED, err.to_string())
        }
        ClientError::ExtensionSupportRequired(_) => (a2a_nats::error::EXTENSION_SUPPORT_REQUIRED, err.to_string()),
        ClientError::VersionNotSupported(_) => (a2a_nats::error::VERSION_NOT_SUPPORTED, err.to_string()),
        ClientError::AgentUnavailable => (a2a_nats::error::AGENT_UNAVAILABLE, err.to_string()),
        ClientError::JsonRpc { code, message } => (*code, message.clone()),
        _ => (INTERNAL_ERROR, err.to_string()),
    }
}
