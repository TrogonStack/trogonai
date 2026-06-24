use futures::StreamExt;
use tracing::{instrument, warn};
use trogon_nats::jetstream::JetStreamPublisher;

use crate::a2a_prefix::A2aPrefix;
use crate::jsonrpc::JsonRpcId;
use crate::nats::subjects::tasks::TaskEventsSubject;
use crate::req_id::ReqId;
use crate::server::handler::{A2aError, A2aExecutor};
use crate::server::wire::{
    encode_error_reply, encode_success_reply, is_notification, parse_request_params, request_id,
};
use crate::task_id::A2aTaskId;

const METHOD: &str = "message/stream";

/// Handles `message/stream`.
///
/// Replies with the initial task envelope as the JSON-RPC bootstrap response and then
/// pumps the handler's [`TaskEventStream`](crate::server::handler::TaskEventStream) onto
/// the JetStream task-events subject until the stream terminates. Cancellation tracking,
/// audit emission, and terminal push-notification dispatch are wired in by the Bridge
/// PR that follows.
#[instrument(
    name = "a2a.server.message_stream",
    skip(handler, headers, payload, reply_subject, nats, js, prefix)
)]
pub async fn handle<H, N, J>(
    handler: &H,
    headers: &async_nats::header::HeaderMap,
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

    if request_id(headers).is_none() && is_notification(headers) {
        return;
    }

    let id = request_id(headers);
    let prepared = prepare_bootstrap(handler, headers, payload, &id).await;
    let (task, mut events, req_id) = match prepared {
        Ok(triple) => triple,
        Err(err) => {
            publish_wire_reply(
                nats,
                &reply,
                encode_error_reply(headers, err.code, err.message, None),
                "message/stream error reply",
            )
            .await;
            return;
        }
    };

    let task_id = match A2aTaskId::new(task.id.clone()) {
        Ok(t) => t,
        Err(_) => {
            let err = A2aError::invalid_agent_response("handler returned task with invalid id");
            publish_wire_reply(
                nats,
                &reply,
                encode_error_reply(headers, err.code, err.message, None),
                "message/stream error reply",
            )
            .await;
            return;
        }
    };

    let bootstrap_envelope = a2a::types::SendMessageResponse::Task(task.clone());
    if !publish_wire_reply(
        nats,
        &reply,
        encode_success_reply(headers, &bootstrap_envelope),
        "message/stream bootstrap reply",
    )
    .await
    {
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
    headers: &async_nats::header::HeaderMap,
    payload: &[u8],
    id: &Option<JsonRpcId>,
) -> Result<(a2a::types::Task, crate::server::handler::TaskEventStream, ReqId), A2aError> {
    let raw = parse_request_params::<serde_json::Value>(METHOD, headers, payload)
        .map_err(|_| A2aError::new(-32700, "Parse error"))?;
    let req_id = match id {
        Some(JsonRpcId::String(s)) => ReqId::from_header(s.clone()),
        _ => {
            return Err(A2aError::new(
                -32602,
                "Invalid params: message/stream requires a string JSON-RPC id (client req_id)",
            ));
        }
    };
    if raw.is_null() {
        return Err(A2aError::new(-32602, "Invalid params: missing params"));
    }
    let req = serde_json::from_value::<a2a::types::SendMessageRequest>(raw)
        .map_err(|e| A2aError::new(-32602, format!("Invalid params: {e}")))?;
    let (task, events) = handler.message_stream(req).await?;
    Ok((task, events, req_id))
}

/// Publish a wire-encoded reply. Returns `true` if the publish succeeded.
async fn publish_wire_reply<N: trogon_nats::PublishClient>(
    nats: &N,
    reply: &str,
    encoded: Result<jsonrpc_nats::Encoded, crate::wire::WireError>,
    label: &'static str,
) -> bool {
    let encoded = match encoded {
        Ok(v) => v,
        Err(e) => {
            warn!(error = %e, label, "failed to encode message/stream payload");
            return false;
        }
    };
    match nats
        .publish_with_headers(
            async_nats::Subject::from(reply),
            encoded.headers,
            encoded.body,
        )
        .await
    {
        Ok(()) => true,
        Err(e) => {
            warn!(error = %e, label, "failed to publish message/stream payload");
            false
        }
    }
}

#[cfg(test)]
mod tests;
