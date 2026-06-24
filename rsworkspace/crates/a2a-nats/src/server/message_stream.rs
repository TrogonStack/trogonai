use futures::StreamExt;
use tracing::{instrument, warn};
use trogon_nats::jetstream::JetStreamPublisher;

use crate::a2a_prefix::A2aPrefix;
use crate::jsonrpc::extract_request_id;
use crate::nats::subjects::tasks::TaskEventsSubject;
use crate::req_id::ReqId;
use crate::server::handler::{A2aError, A2aExecutor};
use crate::server::wire::{JsonRpcErrorResponse, JsonRpcResponse, is_notification, parse_request};
use crate::task_id::A2aTaskId;

/// Handles `message/stream`.
///
/// Replies with the initial task envelope as the JSON-RPC bootstrap response and then
/// pumps the handler's [`TaskEventStream`](crate::server::handler::TaskEventStream) onto
/// the JetStream task-events subject until the stream terminates. Cancellation tracking,
/// audit emission, and terminal push-notification dispatch are wired in by the Bridge
/// PR that follows.
#[instrument(
    name = "a2a.server.message_stream",
    skip(handler, payload, reply_subject, nats, js, prefix)
)]
pub async fn handle<H, N, J>(
    handler: &H,
    payload: &[u8],
    reply_subject: Option<String>,
    nats: &N,
    js: &J,
    prefix: &A2aPrefix,
) where
    H: A2aExecutor,
    N: trogon_nats::PublishClient,
    J: JetStreamPublisher,
{
    let Some(reply) = reply_subject else {
        warn!("message/stream received without reply subject; dropping");
        return;
    };

    let id = extract_request_id(payload);
    if id.is_none() && is_notification(payload) {
        return;
    }

    let prepared = prepare_bootstrap(handler, payload, &id).await;
    let (task, mut events, req_id) = match prepared {
        Ok(triple) => triple,
        Err(err) => {
            let bytes = JsonRpcErrorResponse::new(id, err.code, err.message).to_bytes();
            publish(nats, &reply, bytes, "message/stream error reply").await;
            return;
        }
    };

    let task_id = match A2aTaskId::new(task.id.clone()) {
        Ok(t) => t,
        Err(_) => {
            let err = A2aError::invalid_agent_response("handler returned task with invalid id");
            let bytes = JsonRpcErrorResponse::new(id, err.code, err.message).to_bytes();
            publish(nats, &reply, bytes, "message/stream error reply").await;
            return;
        }
    };

    // The client deserializes message/stream's bootstrap reply as
    // `SendMessageResponse` (same shape as `message/send`), which wraps the
    // task under a `task` key. Wrap here so a bare Task doesn't fail
    // client-side deserialization and bury the stream.
    let bootstrap_envelope = a2a::types::SendMessageResponse::Task(task.clone());
    let bootstrap_bytes = JsonRpcResponse::new(id, &bootstrap_envelope).to_bytes();
    if !publish(nats, &reply, bootstrap_bytes, "message/stream bootstrap reply").await {
        return;
    }

    let events_subject = TaskEventsSubject::new(prefix, &task_id, &req_id).to_string();
    while let Some(item) = events.next().await {
        let payload = match item {
            Ok(event) => match serde_json::to_vec(&event) {
                Ok(b) => bytes::Bytes::from(b),
                Err(e) => {
                    warn!(error = %e, "failed to serialize task event; ending stream");
                    return;
                }
            },
            Err(e) => {
                warn!(error = %e, "task event stream yielded error; ending stream");
                return;
            }
        };
        let subject = async_nats::Subject::from(events_subject.as_str());
        if let Err(e) = js
            .publish_with_headers(subject, async_nats::HeaderMap::new(), payload)
            .await
        {
            warn!(error = %e, "failed to publish task event to JetStream; ending stream");
            return;
        }
    }
}

async fn prepare_bootstrap<H: A2aExecutor>(
    handler: &H,
    payload: &[u8],
    id: &Option<crate::jsonrpc::JsonRpcId>,
) -> Result<(a2a::types::Task, crate::server::handler::TaskEventStream, ReqId), A2aError> {
    let envelope = parse_request::<serde_json::Value>(payload).map_err(|_| A2aError::new(-32700, "Parse error"))?;
    // The client subscribes to `{prefix}.tasks.*.events.{req_id}` using the JSON-RPC
    // `id` string it sent; we must publish on that exact suffix or the consumer's
    // filter rejects every event. Validate before calling the handler so a bad id
    // doesn't burn a stream construction we can't ever route.
    let req_id = match id {
        Some(crate::jsonrpc::JsonRpcId::String(s)) => ReqId::from_header(s.clone()),
        _ => {
            return Err(A2aError::new(
                -32602,
                "Invalid params: message/stream requires a string JSON-RPC id (client req_id)",
            ));
        }
    };
    let raw = envelope
        .params
        .ok_or_else(|| A2aError::new(-32602, "Invalid params: missing params"))?;
    let req = serde_json::from_value::<a2a::types::SendMessageRequest>(raw)
        .map_err(|e| A2aError::new(-32602, format!("Invalid params: {e}")))?;
    let (task, events) = handler.message_stream(req).await?;
    Ok((task, events, req_id))
}

/// Publish `bytes` to `reply`. Returns `true` if the publish succeeded.
/// `label` flows into the warn message so the failing operation is identifiable.
async fn publish<N: trogon_nats::PublishClient>(
    nats: &N,
    reply: &str,
    bytes: Result<bytes::Bytes, serde_json::Error>,
    label: &'static str,
) -> bool {
    let bytes = match bytes {
        Ok(b) => b,
        Err(e) => {
            warn!(error = %e, label = label, "failed to serialize message/stream payload");
            return false;
        }
    };
    let headers = async_nats::HeaderMap::new();
    if let Err(e) = nats
        .publish_with_headers(async_nats::Subject::from(reply), headers, bytes)
        .await
    {
        warn!(error = %e, label = label, "failed to publish message/stream payload");
        return false;
    }
    true
}

#[cfg(test)]
mod tests;
