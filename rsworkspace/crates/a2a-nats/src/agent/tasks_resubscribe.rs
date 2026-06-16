use tracing::{instrument, warn};

use crate::agent::handler::{A2aError, A2aHandler};
use crate::agent::wire::{JsonRpcErrorResponse, JsonRpcResponse, parse_request};
use crate::jsonrpc::JsonRpcId;

/// Handles `tasks/resubscribe`.
///
/// The agent validates the task ID by delegating to the handler (which returns the current
/// task snapshot), then replies with that snapshot. The client is then responsible for
/// creating a JetStream consumer via `resubscribe_consumer()` to replay missed events
/// from the `last_seq` it supplies. The agent does not replay events itself.
#[instrument(name = "a2a.agent.tasks_resubscribe", skip(handler, payload, reply_subject, nats))]
pub async fn handle<H, N>(handler: &H, payload: &[u8], reply_subject: Option<String>, nats: &N)
where
    H: A2aHandler,
    N: trogon_nats::PublishClient,
{
    let Some(reply) = reply_subject else {
        warn!("tasks/resubscribe received without reply subject; dropping");
        return;
    };

    let (id, result) = parse_and_call(handler, payload).await;
    let bytes = match result {
        Ok(resp) => JsonRpcResponse::new(id, resp).to_bytes(),
        Err(e) => JsonRpcErrorResponse::new(id, e.code, e.message).to_bytes(),
    };
    match bytes {
        Ok(b) => {
            let headers = async_nats::HeaderMap::new();
            if let Err(e) = nats
                .publish_with_headers(async_nats::Subject::from(reply.as_str()), headers, b)
                .await
            {
                warn!(error = %e, "failed to publish tasks/resubscribe reply");
            }
        }
        Err(e) => warn!(error = %e, "failed to serialize tasks/resubscribe response"),
    }
}

async fn parse_and_call<H: A2aHandler>(
    handler: &H,
    payload: &[u8],
) -> (Option<JsonRpcId>, Result<a2a_types::Task, A2aError>) {
    let req = match parse_request::<a2a_types::SubscribeToTaskRequest>(payload) {
        Ok(r) => r,
        Err(_) => return (None, Err(A2aError::internal("parse error"))),
    };
    let id = req.id;
    let params = match req.params {
        Some(p) => p,
        None => return (id, Err(A2aError::internal("missing params"))),
    };
    (id, handler.tasks_resubscribe(params).await)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::test_support::{make_task, parse_response, rpc_payload, stub};
    use trogon_nats::AdvancedMockNatsClient;

    #[tokio::test]
    async fn success_publishes_task_snapshot() {
        let nats = AdvancedMockNatsClient::new();
        let handler = stub();
        handler.lock().unwrap().tasks_resubscribe_result = Some(Ok(make_task("t-resub")));
        handle(
            &handler,
            &rpc_payload("tasks/resubscribe", 1),
            Some("reply".into()),
            &nats,
        )
        .await;
        let body = parse_response(&nats.published_payloads()[0]);
        assert_eq!(body["result"]["id"], "t-resub");
    }

    #[tokio::test]
    async fn task_not_found_error() {
        let nats = AdvancedMockNatsClient::new();
        let handler = stub();
        handler.lock().unwrap().tasks_resubscribe_result = Some(Err(A2aError::task_not_found("gone")));
        handle(
            &handler,
            &rpc_payload("tasks/resubscribe", 2),
            Some("reply".into()),
            &nats,
        )
        .await;
        let body = parse_response(&nats.published_payloads()[0]);
        assert_eq!(body["error"]["code"], crate::error::TASK_NOT_FOUND);
    }

    #[tokio::test]
    async fn no_reply_drops() {
        let nats = AdvancedMockNatsClient::new();
        let handler = stub();
        handle(&handler, &rpc_payload("tasks/resubscribe", 3), None, &nats).await;
        assert!(nats.published_messages().is_empty());
    }
}
