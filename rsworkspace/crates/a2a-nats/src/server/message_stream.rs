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

    let req = match parse_request::<serde_json::Value>(payload) {
        Err(_) => {
            publish_bootstrap_error(nats, &reply, id, A2aError::new(-32700, "Parse error")).await;
            return;
        }
        Ok(envelope) => match envelope.params {
            None => {
                publish_bootstrap_error(
                    nats,
                    &reply,
                    id,
                    A2aError::new(-32602, "Invalid params: missing params"),
                )
                .await;
                return;
            }
            Some(raw) => match serde_json::from_value::<a2a::types::SendMessageRequest>(raw) {
                Err(e) => {
                    publish_bootstrap_error(nats, &reply, id, A2aError::new(-32602, format!("Invalid params: {e}")))
                        .await;
                    return;
                }
                Ok(p) => p,
            },
        },
    };

    let (task, mut events) = match handler.message_stream(req).await {
        Ok(pair) => pair,
        Err(e) => {
            publish_bootstrap_error(nats, &reply, id, e).await;
            return;
        }
    };

    let task_id = match A2aTaskId::new(task.id.clone()) {
        Ok(t) => t,
        Err(_) => {
            publish_bootstrap_error(
                nats,
                &reply,
                id,
                A2aError::invalid_agent_response("handler returned task with invalid id"),
            )
            .await;
            return;
        }
    };
    // Bridge PR threads the caller's req id from the gateway-ingress headers; until
    // that landed, fall back to a freshly minted server-side id.
    let req_id = ReqId::new();

    let bootstrap = match JsonRpcResponse::new(id, &task).to_bytes() {
        Ok(b) => b,
        Err(e) => {
            warn!(error = %e, "failed to serialize message/stream bootstrap reply");
            return;
        }
    };
    let headers = async_nats::HeaderMap::new();
    if let Err(e) = nats
        .publish_with_headers(async_nats::Subject::from(reply.as_str()), headers, bootstrap)
        .await
    {
        warn!(error = %e, "failed to publish message/stream bootstrap reply");
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

async fn publish_bootstrap_error<N: trogon_nats::PublishClient>(
    nats: &N,
    reply: &str,
    id: Option<crate::jsonrpc::JsonRpcId>,
    err: A2aError,
) {
    match JsonRpcErrorResponse::new(id, err.code, err.message).to_bytes() {
        Ok(b) => {
            let headers = async_nats::HeaderMap::new();
            if let Err(e) = nats
                .publish_with_headers(async_nats::Subject::from(reply), headers, b)
                .await
            {
                warn!(error = %e, "failed to publish message/stream error reply");
            }
        }
        Err(e) => warn!(error = %e, "failed to serialize message/stream error reply"),
    }
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

    fn stream_payload(req_id: i64) -> Vec<u8> {
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

        handle(&handler, &stream_payload(1), Some("r".into()), &nats, &js, &prefix()).await;

        let body = parse_response(&nats.published_payloads()[0]);
        assert_eq!(body["result"]["id"].as_str(), Some("task-1"));
        let subjects = js.published_subjects();
        assert_eq!(subjects.len(), 1);
        assert!(
            subjects[0].starts_with("a2a.tasks.task-1.events."),
            "unexpected subject {}",
            subjects[0]
        );
    }

    #[tokio::test]
    async fn handler_error_published_as_bootstrap_error() {
        let nats = AdvancedMockNatsClient::new();
        let js = MockJetStreamPublisher::new();
        let handler = stub();
        handler.lock().unwrap().message_stream_result = Some(Err(A2aError::agent_unavailable("down")));

        handle(&handler, &stream_payload(2), Some("r".into()), &nats, &js, &prefix()).await;

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
        handle(&handler, &stream_payload(3), None, &nats, &js, &prefix()).await;
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
    async fn invalid_task_id_from_handler_returns_invalid_agent_response() {
        let nats = AdvancedMockNatsClient::new();
        let js = MockJetStreamPublisher::new();
        let handler = stub();
        let events: crate::server::handler::TaskEventStream = Box::pin(stream::iter(vec![]));
        handler.lock().unwrap().message_stream_result = Some(Ok((task(""), events)));

        handle(&handler, &stream_payload(8), Some("r".into()), &nats, &js, &prefix()).await;

        let body = parse_response(&nats.published_payloads()[0]);
        assert_eq!(
            body["error"]["code"].as_i64(),
            Some(i64::from(crate::error::INVALID_AGENT_RESPONSE))
        );
    }
}
