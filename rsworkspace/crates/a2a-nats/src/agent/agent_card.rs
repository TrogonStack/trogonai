use tracing::{instrument, warn};

use crate::agent::handler::{A2aError, A2aHandler};
use crate::agent::wire::{JsonRpcErrorResponse, JsonRpcResponse, parse_request};
use crate::jsonrpc::JsonRpcId;

#[instrument(name = "a2a.agent.agent_card", skip(handler, payload, reply_subject, nats))]
pub async fn handle<H, N>(handler: &H, payload: &[u8], reply_subject: Option<String>, nats: &N)
where
    H: A2aHandler,
    N: trogon_nats::PublishClient,
{
    let Some(reply) = reply_subject else {
        warn!("agent/getAuthenticatedExtendedCard received without reply subject; dropping");
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
                warn!(error = %e, "failed to publish agent_card reply");
            }
        }
        Err(e) => warn!(error = %e, "failed to serialize agent_card response"),
    }
}

async fn parse_and_call<H: A2aHandler>(
    handler: &H,
    payload: &[u8],
) -> (Option<JsonRpcId>, Result<a2a_types::AgentCard, A2aError>) {
    let req = match parse_request::<a2a_types::GetExtendedAgentCardRequest>(payload) {
        Ok(r) => r,
        Err(_) => return (None, Err(A2aError::internal("parse error"))),
    };
    let id = req.id;
    let params = req.params.unwrap_or_default();
    (id, handler.agent_card(params).await)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::test_support::{parse_response, rpc_payload, stub};
    use trogon_nats::AdvancedMockNatsClient;

    #[tokio::test]
    async fn success_publishes_agent_card() {
        let nats = AdvancedMockNatsClient::new();
        let handler = stub();
        handler.lock().unwrap().agent_card_result = Some(Ok(a2a_types::AgentCard {
            name: "my-agent".into(),
            ..Default::default()
        }));
        handle(
            &handler,
            &rpc_payload("agent/getAuthenticatedExtendedCard", 1),
            Some("r".into()),
            &nats,
        )
        .await;
        let body = parse_response(&nats.published_payloads()[0]);
        assert_eq!(body["result"]["name"], "my-agent");
    }

    #[tokio::test]
    async fn error_response() {
        let nats = AdvancedMockNatsClient::new();
        let handler = stub();
        handler.lock().unwrap().agent_card_result = Some(Err(A2aError::unsupported_operation("no card")));
        handle(
            &handler,
            &rpc_payload("agent/getAuthenticatedExtendedCard", 2),
            Some("r".into()),
            &nats,
        )
        .await;
        let body = parse_response(&nats.published_payloads()[0]);
        assert_eq!(body["error"]["code"], crate::error::UNSUPPORTED_OPERATION);
    }

    #[tokio::test]
    async fn no_reply_drops() {
        let nats = AdvancedMockNatsClient::new();
        let handler = stub();
        handle(
            &handler,
            &rpc_payload("agent/getAuthenticatedExtendedCard", 3),
            None,
            &nats,
        )
        .await;
        assert!(nats.published_messages().is_empty());
    }

    #[tokio::test]
    async fn request_without_params_still_calls_handler() {
        let nats = AdvancedMockNatsClient::new();
        let handler = stub();
        handler.lock().unwrap().agent_card_result = Some(Ok(a2a_types::AgentCard::default()));
        let payload = serde_json::to_vec(
            &serde_json::json!({"jsonrpc":"2.0","id":1,"method":"agent/getAuthenticatedExtendedCard"}),
        )
        .unwrap();
        handle(&handler, &payload, Some("r".into()), &nats).await;
        let body = parse_response(&nats.published_payloads()[0]);
        assert!(body.get("result").is_some());
    }
}
