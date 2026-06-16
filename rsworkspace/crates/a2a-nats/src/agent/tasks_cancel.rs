use tracing::{instrument, warn};

use crate::agent::handler::{A2aError, A2aHandler};
use crate::agent::wire::{JsonRpcErrorResponse, JsonRpcResponse, parse_request};
use crate::jsonrpc::JsonRpcId;

#[instrument(name = "a2a.agent.tasks_cancel", skip(handler, payload, reply_subject, nats))]
pub async fn handle<H, N>(handler: &H, payload: &[u8], reply_subject: Option<String>, nats: &N)
where
    H: A2aHandler,
    N: trogon_nats::PublishClient,
{
    let Some(reply) = reply_subject else {
        warn!("tasks/cancel received without reply subject; dropping");
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
                warn!(error = %e, "failed to publish tasks/cancel reply");
            }
        }
        Err(e) => warn!(error = %e, "failed to serialize tasks/cancel response"),
    }
}

async fn parse_and_call<H: A2aHandler>(
    handler: &H,
    payload: &[u8],
) -> (Option<JsonRpcId>, Result<a2a_types::Task, A2aError>) {
    let req = match parse_request::<a2a_types::CancelTaskRequest>(payload) {
        Ok(r) => r,
        Err(_) => return (None, Err(A2aError::internal("parse error"))),
    };
    let id = req.id;
    let params = match req.params {
        Some(p) => p,
        None => return (id, Err(A2aError::internal("missing params"))),
    };
    (id, handler.tasks_cancel(params).await)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::test_support::{make_task, parse_response, rpc_payload, stub};
    use trogon_nats::AdvancedMockNatsClient;

    #[tokio::test]
    async fn success_publishes_canceled_task() {
        let nats = AdvancedMockNatsClient::new();
        let handler = stub();
        handler.lock().unwrap().tasks_cancel_result = Some(Ok(make_task("t-cancel")));
        handle(&handler, &rpc_payload("tasks/cancel", 1), Some("reply".into()), &nats).await;
        let body = parse_response(&nats.published_payloads()[0]);
        assert_eq!(body["result"]["id"], "t-cancel");
    }

    #[tokio::test]
    async fn not_cancelable_error() {
        let nats = AdvancedMockNatsClient::new();
        let handler = stub();
        handler.lock().unwrap().tasks_cancel_result = Some(Err(A2aError::task_not_cancelable("already done")));
        handle(&handler, &rpc_payload("tasks/cancel", 2), Some("reply".into()), &nats).await;
        let body = parse_response(&nats.published_payloads()[0]);
        assert_eq!(body["error"]["code"], crate::error::TASK_NOT_CANCELABLE);
    }

    #[tokio::test]
    async fn no_reply_drops() {
        let nats = AdvancedMockNatsClient::new();
        let handler = stub();
        handle(&handler, &rpc_payload("tasks/cancel", 3), None, &nats).await;
        assert!(nats.published_messages().is_empty());
    }
}
