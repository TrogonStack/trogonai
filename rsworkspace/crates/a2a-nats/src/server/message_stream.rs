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
mod tests {
    use futures::stream;
    use trogon_nats::AdvancedMockNatsClient;
    use trogon_nats::jetstream::mocks::MockJetStreamPublisher;

    use super::*;
    use crate::server::test_support::{parse_response, stub};

    fn prefix() -> A2aPrefix {
        A2aPrefix::new("a2a").unwrap()
    }

    fn stream_payload(req_id: &str) -> Vec<u8> {
        serde_json::to_vec(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": req_id,
            "method": "message/stream",
            "params": {
                "message": {
                    "messageId": "m-1",
                    "role": "ROLE_USER",
                    "parts": []
                }
            }
        }))
        .unwrap()
    }

    fn stream_payload_numeric_id(req_id: i64) -> Vec<u8> {
        serde_json::to_vec(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": req_id,
            "method": "message/stream",
            "params": {
                "message": {
                    "messageId": "m-1",
                    "role": "ROLE_USER",
                    "parts": []
                }
            }
        }))
        .unwrap()
    }

    fn task(task_id: &str) -> a2a::types::Task {
        a2a::types::Task {
            id: task_id.to_string(),
            context_id: String::new(),
            status: a2a::types::TaskStatus {
                state: a2a::types::TaskState::Working,
                message: None,
                timestamp: None,
            },
            artifacts: None,
            history: None,
            metadata: None,
        }
    }

    fn working_status_event(task_id: &str) -> a2a::event::StreamResponse {
        a2a::event::StreamResponse::StatusUpdate(a2a::event::TaskStatusUpdateEvent {
            task_id: task_id.to_string(),
            context_id: "ctx".to_string(),
            status: a2a::types::TaskStatus {
                state: a2a::types::TaskState::Working,
                message: None,
                timestamp: None,
            },
            metadata: None,
        })
    }

    #[tokio::test]
    async fn bootstrap_publishes_task_and_then_events() {
        let nats = AdvancedMockNatsClient::new();
        let js = MockJetStreamPublisher::new();
        let handler = stub();
        let events: crate::server::handler::TaskEventStream =
            Box::pin(stream::iter(vec![Ok(working_status_event("task-1"))]));
        handler.lock().unwrap().message_stream_result = Some(Ok((task("task-1"), events)));

        handle(
            &handler,
            &stream_payload("call-1"),
            Some("r".into()),
            &nats,
            &js,
            &prefix(),
        )
        .await;

        let body = parse_response(&nats.published_payloads()[0]);
        assert_eq!(body["result"]["task"]["id"].as_str(), Some("task-1"));
        let subjects = js.published_subjects();
        // The event subject MUST use the caller's JSON-RPC id as the suffix so it
        // matches `stream_events_consumer`'s `{prefix}.tasks.*.events.{req_id}` filter
        // on the client side; otherwise streamed events never reach the subscriber.
        assert_eq!(subjects, vec!["a2a.tasks.task-1.events.call-1".to_string()]);
    }

    #[tokio::test]
    async fn numeric_jsonrpc_id_rejected_with_invalid_params() {
        let nats = AdvancedMockNatsClient::new();
        let js = MockJetStreamPublisher::new();
        let handler = stub();
        // Handler should not even be called; if it were, this would yield a panic on
        // the unwrap-stub default. We just want to confirm the error reply shape.
        handle(
            &handler,
            &stream_payload_numeric_id(42),
            Some("r".into()),
            &nats,
            &js,
            &prefix(),
        )
        .await;
        let body = parse_response(&nats.published_payloads()[0]);
        assert_eq!(body["error"]["code"], -32602);
        assert_eq!(body["id"], 42);
        assert!(js.published_subjects().is_empty());
    }

    #[tokio::test]
    async fn handler_error_published_as_bootstrap_error() {
        let nats = AdvancedMockNatsClient::new();
        let js = MockJetStreamPublisher::new();
        let handler = stub();
        handler.lock().unwrap().message_stream_result = Some(Err(A2aError::agent_unavailable("down")));

        handle(
            &handler,
            &stream_payload("call-2"),
            Some("r".into()),
            &nats,
            &js,
            &prefix(),
        )
        .await;

        let body = parse_response(&nats.published_payloads()[0]);
        assert_eq!(
            body["error"]["code"].as_i64(),
            Some(i64::from(crate::error::AGENT_UNAVAILABLE))
        );
        assert!(js.published_subjects().is_empty());
    }

    #[tokio::test]
    async fn no_reply_drops_request() {
        let nats = AdvancedMockNatsClient::new();
        let js = MockJetStreamPublisher::new();
        let handler = stub();
        handle(&handler, &stream_payload("call-3"), None, &nats, &js, &prefix()).await;
        assert!(nats.published_messages().is_empty());
    }

    #[tokio::test]
    async fn missing_params_returns_invalid_params_error() {
        let nats = AdvancedMockNatsClient::new();
        let js = MockJetStreamPublisher::new();
        let handler = stub();
        let payload = serde_json::to_vec(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": 5,
            "method": "message/stream"
        }))
        .unwrap();
        handle(&handler, &payload, Some("r".into()), &nats, &js, &prefix()).await;
        let body = parse_response(&nats.published_payloads()[0]);
        assert_eq!(body["error"]["code"], -32602);
    }

    #[tokio::test]
    async fn invalid_params_shape_returns_invalid_params_code() {
        let nats = AdvancedMockNatsClient::new();
        let js = MockJetStreamPublisher::new();
        let handler = stub();
        let payload = serde_json::to_vec(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": 6,
            "method": "message/stream",
            "params": {"message": 42}
        }))
        .unwrap();
        handle(&handler, &payload, Some("r".into()), &nats, &js, &prefix()).await;
        let body = parse_response(&nats.published_payloads()[0]);
        assert_eq!(body["error"]["code"], -32602);
        assert_eq!(body["id"], 6);
    }

    #[tokio::test]
    async fn malformed_json_still_publishes_parse_error_with_null_id() {
        let nats = AdvancedMockNatsClient::new();
        let js = MockJetStreamPublisher::new();
        let handler = stub();
        handle(&handler, b"not json", Some("r".into()), &nats, &js, &prefix()).await;
        let body = parse_response(&nats.published_payloads()[0]);
        assert_eq!(body["error"]["code"], -32700);
        assert!(body["id"].is_null());
    }

    #[tokio::test]
    async fn notification_without_id_is_dropped() {
        let nats = AdvancedMockNatsClient::new();
        let js = MockJetStreamPublisher::new();
        let handler = stub();
        let payload = serde_json::to_vec(&serde_json::json!({
            "jsonrpc": "2.0",
            "method": "message/stream",
            "params": {"message": {"messageId":"m","role":"ROLE_USER","parts":[]}}
        }))
        .unwrap();
        handle(&handler, &payload, Some("r".into()), &nats, &js, &prefix()).await;
        assert!(nats.published_messages().is_empty());
    }

    #[tokio::test]
    async fn publish_returns_false_when_serialize_fails() {
        let nats = AdvancedMockNatsClient::new();
        let err: Result<bytes::Bytes, serde_json::Error> = Err(serde_json::from_str::<String>("x").unwrap_err());
        let ok = publish(&nats, "r", err, "test-label").await;
        assert!(!ok);
        assert!(nats.published_messages().is_empty());
    }

    #[tokio::test]
    async fn publish_returns_false_when_nats_publish_fails() {
        let nats = AdvancedMockNatsClient::new();
        nats.fail_next_publish();
        let ok = publish(&nats, "r", Ok(bytes::Bytes::from_static(b"{}")), "test-label").await;
        assert!(!ok);
    }

    #[tokio::test]
    async fn invalid_task_id_from_handler_returns_invalid_agent_response() {
        let nats = AdvancedMockNatsClient::new();
        let js = MockJetStreamPublisher::new();
        let handler = stub();
        let events: crate::server::handler::TaskEventStream = Box::pin(stream::iter(vec![]));
        handler.lock().unwrap().message_stream_result = Some(Ok((task(""), events)));

        handle(
            &handler,
            &stream_payload("call-8"),
            Some("r".into()),
            &nats,
            &js,
            &prefix(),
        )
        .await;

        let body = parse_response(&nats.published_payloads()[0]);
        assert_eq!(
            body["error"]["code"].as_i64(),
            Some(i64::from(crate::error::INVALID_AGENT_RESPONSE))
        );
    }
}
