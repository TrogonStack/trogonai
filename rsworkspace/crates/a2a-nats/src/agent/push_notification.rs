use tracing::{instrument, warn};

use crate::agent::handler::{A2aError, A2aHandler};
use crate::agent::wire::{JsonRpcErrorResponse, JsonRpcResponse, parse_request};
use crate::jsonrpc::JsonRpcId;

macro_rules! unary_handler {
    ($fn_name:ident, $span:literal, $req_ty:ty, $res_ty:ty, $method:ident) => {
        #[instrument(name = $span, skip(handler, payload, reply_subject, nats))]
        pub async fn $fn_name<H, N>(handler: &H, payload: &[u8], reply_subject: Option<String>, nats: &N)
        where
            H: A2aHandler,
            N: trogon_nats::PublishClient,
        {
            let Some(reply) = reply_subject else {
                warn!(concat!($span, " received without reply subject; dropping"));
                return;
            };

            let (id, result) = unary_parse_and_call::<H, $req_ty, $res_ty>(handler, payload, |h, p| {
                Box::pin(h.$method(p))
            })
            .await;

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
                        warn!(error = %e, concat!("failed to publish ", $span, " reply"));
                    }
                }
                Err(e) => warn!(error = %e, concat!("failed to serialize ", $span, " response")),
            }
        }
    };
}

async fn unary_parse_and_call<H, P, R>(
    handler: &H,
    payload: &[u8],
    call: impl FnOnce(&H, P) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<R, A2aError>> + Send + '_>>,
) -> (Option<JsonRpcId>, Result<R, A2aError>)
where
    H: A2aHandler,
    P: serde::de::DeserializeOwned,
{
    let req = match parse_request::<P>(payload) {
        Ok(r) => r,
        Err(_) => return (None, Err(A2aError::internal("parse error"))),
    };
    let id = req.id;
    let params = match req.params {
        Some(p) => p,
        None => return (id, Err(A2aError::internal("missing params"))),
    };
    (id, call(handler, params).await)
}

unary_handler!(
    handle_set,
    "a2a.agent.push_notification_set",
    a2a_types::TaskPushNotificationConfig,
    a2a_types::TaskPushNotificationConfig,
    push_notification_set
);

unary_handler!(
    handle_get,
    "a2a.agent.push_notification_get",
    a2a_types::GetTaskPushNotificationConfigRequest,
    a2a_types::TaskPushNotificationConfig,
    push_notification_get
);

unary_handler!(
    handle_list,
    "a2a.agent.push_notification_list",
    a2a_types::ListTaskPushNotificationConfigsRequest,
    a2a_types::ListTaskPushNotificationConfigsResponse,
    push_notification_list
);

#[instrument(name = "a2a.agent.push_notification_delete", skip(handler, payload, reply_subject, nats))]
pub async fn handle_delete<H, N>(handler: &H, payload: &[u8], reply_subject: Option<String>, nats: &N)
where
    H: A2aHandler,
    N: trogon_nats::PublishClient,
{
    let Some(reply) = reply_subject else {
        warn!("push_notification_delete received without reply subject; dropping");
        return;
    };

    let req = match parse_request::<a2a_types::DeleteTaskPushNotificationConfigRequest>(payload) {
        Ok(r) => r,
        Err(_) => {
            send_error(nats, &reply, None, A2aError::internal("parse error")).await;
            return;
        }
    };
    let id = req.id.clone();
    let params = match req.params {
        Some(p) => p,
        None => {
            send_error(nats, &reply, id, A2aError::internal("missing params")).await;
            return;
        }
    };

    match handler.push_notification_delete(params).await {
        Ok(()) => {
            let bytes = JsonRpcResponse::new(id, serde_json::Value::Null).to_bytes();
            match bytes {
                Ok(b) => {
                    let headers = async_nats::HeaderMap::new();
                    if let Err(e) = nats
                        .publish_with_headers(async_nats::Subject::from(reply.as_str()), headers, b)
                        .await
                    {
                        warn!(error = %e, "failed to publish push_notification_delete reply");
                    }
                }
                Err(e) => warn!(error = %e, "failed to serialize push_notification_delete response"),
            }
        }
        Err(e) => send_error(nats, &reply, req.id, e).await,
    }
}

async fn send_error<N: trogon_nats::PublishClient>(
    nats: &N,
    reply: &str,
    id: Option<JsonRpcId>,
    err: A2aError,
) {
    let bytes = JsonRpcErrorResponse::new(id, err.code, err.message).to_bytes();
    match bytes {
        Ok(b) => {
            let headers = async_nats::HeaderMap::new();
            if let Err(e) = nats
                .publish_with_headers(async_nats::Subject::from(reply), headers, b)
                .await
            {
                warn!(error = %e, "failed to publish push error reply");
            }
        }
        Err(e) => warn!(error = %e, "failed to serialize push error response"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::test_support::{parse_response, rpc_payload, stub};
    use trogon_nats::AdvancedMockNatsClient;

    #[tokio::test]
    async fn set_success() {
        let nats = AdvancedMockNatsClient::new();
        let handler = stub();
        handler.lock().unwrap().push_set_result =
            Some(Ok(a2a_types::TaskPushNotificationConfig { id: "cfg1".into(), ..Default::default() }));
        handle_set(&handler, &rpc_payload("tasks/pushNotificationConfig/set", 1), Some("r".into()), &nats).await;
        let body = parse_response(&nats.published_payloads()[0]);
        assert_eq!(body["result"]["id"], "cfg1");
    }

    #[tokio::test]
    async fn get_not_supported() {
        let nats = AdvancedMockNatsClient::new();
        let handler = stub();
        handler.lock().unwrap().push_get_result = Some(Err(A2aError::push_notification_not_supported("no")));
        handle_get(&handler, &rpc_payload("tasks/pushNotificationConfig/get", 2), Some("r".into()), &nats).await;
        let body = parse_response(&nats.published_payloads()[0]);
        assert_eq!(body["error"]["code"], crate::error::PUSH_NOTIFICATION_NOT_SUPPORTED);
    }

    #[tokio::test]
    async fn list_success() {
        let nats = AdvancedMockNatsClient::new();
        let handler = stub();
        handler.lock().unwrap().push_list_result =
            Some(Ok(a2a_types::ListTaskPushNotificationConfigsResponse::default()));
        handle_list(&handler, &rpc_payload("tasks/pushNotificationConfig/list", 3), Some("r".into()), &nats).await;
        let body = parse_response(&nats.published_payloads()[0]);
        // pbjson omits empty repeated fields; presence of "result" key confirms success
        assert!(body.get("result").is_some());
    }

    #[tokio::test]
    async fn delete_success_returns_null_result() {
        let nats = AdvancedMockNatsClient::new();
        let handler = stub();
        handler.lock().unwrap().push_delete_result = Some(Ok(()));
        handle_delete(&handler, &rpc_payload("tasks/pushNotificationConfig/delete", 4), Some("r".into()), &nats).await;
        let body = parse_response(&nats.published_payloads()[0]);
        assert_eq!(body["result"], serde_json::Value::Null);
    }

    #[tokio::test]
    async fn no_reply_drops_message() {
        let nats = AdvancedMockNatsClient::new();
        let handler = stub();
        handle_set(&handler, &rpc_payload("tasks/pushNotificationConfig/set", 5), None, &nats).await;
        assert!(nats.published_messages().is_empty());
    }
}
