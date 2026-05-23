use bytes::Bytes;
use tracing::{instrument, warn};

use crate::agent::handler::{A2aError, A2aHandler};
use crate::agent::wire::{JsonRpcErrorResponse, JsonRpcResponse, parse_request};
use crate::jsonrpc::JsonRpcId;
use crate::push::delivery_semantics::{
    DeliverySemanticsParseError, merged_request_delivery_semantics, upsert_delivery_semantics_on_push_config_json_object,
};
use crate::push::PushDeliverySemanticsRegistry;
use crate::push::push_notification_config_id::PushNotificationConfigId;
use crate::task_id::A2aTaskId;

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

async fn reply_success<N: trogon_nats::PublishClient>(
    nats: &N,
    reply: &str,
    id: Option<JsonRpcId>,
    body: serde_json::Value,
) {
    match JsonRpcResponse::new(id, body).to_bytes() {
        Ok(b) => {
            let headers = async_nats::HeaderMap::new();
            if let Err(e) = nats
                .publish_with_headers(async_nats::Subject::from(reply), headers, Bytes::from(b))
                .await
            {
                warn!(error = %e, "failed to publish push_notification success reply");
            }
        }
        Err(e) => warn!(error = %e, "failed to serialize push_notification success reply"),
    }
}

#[instrument(
    name = "a2a.agent.push_notification_set",
    skip(handler, payload, reply_subject, nats, semantics_registry)
)]
pub async fn handle_set<H, N>(
    handler: &H,
    payload: &[u8],
    reply_subject: Option<String>,
    nats: &N,
    semantics_registry: &PushDeliverySemanticsRegistry,
) where
    H: A2aHandler,
    N: trogon_nats::PublishClient,
{
    let Some(reply) = reply_subject else {
        warn!("push_notification_set received without reply subject; dropping");
        return;
    };

    let envelope = match parse_request::<serde_json::Value>(payload) {
        Ok(v) => v,
        Err(_) => {
            send_error(nats, &reply, None, A2aError::internal("parse error")).await;
            return;
        }
    };
    let id = envelope.id.clone();
    let raw_params = match envelope.params.clone() {
        Some(p) => p,
        None => {
            send_error(nats, &reply, id, A2aError::internal("missing params")).await;
            return;
        }
    };

    let serde_json::Value::Object(mut map) = raw_params else {
        send_error(nats, &reply, id.clone(), A2aError::internal("params must be a JSON object")).await;
        return;
    };

    let delivery_sem_json = map.remove("deliverySemantics");
    let exactly_json = map.remove("exactlyOnceDelivery");
    let exactly_bool = exactly_json.and_then(|v| match v {
        serde_json::Value::Bool(b) => Some(b),
        _ => None,
    });
    let requested_semantics =
        match merged_request_delivery_semantics(delivery_sem_json.as_ref(), exactly_bool) {
            Ok(s) => s,
            Err(e) => {
                send_error(nats, &reply, id, map_delivery_semantics_parse_error(e)).await;
                return;
            }
        };

    let proto_cfg: a2a_types::TaskPushNotificationConfig = match serde_json::from_value(serde_json::Value::Object(map)) {
        Ok(c) => c,
        Err(e) => {
            send_error(
                nats,
                &reply,
                id,
                A2aError::internal(format!("failed to deserialize TaskPushNotificationConfig: {e}")),
            )
            .await;
            return;
        }
    };

    match handler.push_notification_set(proto_cfg).await {
        Ok(resp) => {
            let mut body = serde_json::to_value(&resp).unwrap_or(serde_json::Value::Null);
            if let serde_json::Value::Object(ref mut o) = body {
                upsert_delivery_semantics_on_push_config_json_object(o, &requested_semantics);
            }

            match (A2aTaskId::new(resp.task_id.clone()), PushNotificationConfigId::new(resp.id.clone())) {
                (Ok(tid), Ok(cid)) => semantics_registry.set(tid, cid, requested_semantics),
                _ => warn!(
                    task_id = resp.task_id,
                    cfg_id = resp.id.as_str(),
                    "push semantics registry skipped due to malformed ids after successful set",
                ),
            }

            reply_success(nats, &reply, id, body).await;
        }
        Err(e) => send_error(nats, &reply, id, e).await,
    }
}

fn map_delivery_semantics_parse_error(e: DeliverySemanticsParseError) -> A2aError {
    A2aError::internal(format!("deliverySemantics extension invalid: {e}"))
}

fn inject_registry_semantics(
    semantics_registry: &PushDeliverySemanticsRegistry,
    task_id: &str,
    config_object: &mut serde_json::Map<String, serde_json::Value>,
) {
    let Some(task_id_dom) = A2aTaskId::new(task_id.to_owned()).ok() else {
        return;
    };
    let cid_str = match config_object.get("id").and_then(|v| v.as_str()) {
        Some(s) => s,
        None => return,
    };
    let Ok(cid_dom) = PushNotificationConfigId::new(cid_str) else {
        return;
    };

    let sem = semantics_registry.get(&task_id_dom, &cid_dom);
    upsert_delivery_semantics_on_push_config_json_object(config_object, &sem);
}

#[instrument(name = "a2a.agent.push_notification_get", skip(handler, payload, reply_subject, nats, semantics_registry))]
pub async fn handle_get<H, N>(
    handler: &H,
    payload: &[u8],
    reply_subject: Option<String>,
    nats: &N,
    semantics_registry: &PushDeliverySemanticsRegistry,
) where
    H: A2aHandler,
    N: trogon_nats::PublishClient,
{
    let Some(reply) = reply_subject else {
        warn!("push_notification_get received without reply subject; dropping");
        return;
    };

    let (id, result) =
        unary_parse_and_call(handler, payload, |h, p| Box::pin(h.push_notification_get(p))).await;

    match result {
        Ok(resp) => {
            let mut body = serde_json::to_value(&resp).unwrap_or(serde_json::Value::Null);
            if let serde_json::Value::Object(ref mut o) = body {
                inject_registry_semantics(semantics_registry, resp.task_id.as_str(), o);
            }

            reply_success(nats, &reply, id, body).await;
        }
        Err(e) => send_error(nats, &reply, id, e).await,
    }
}

#[instrument(name = "a2a.agent.push_notification_list", skip(handler, payload, reply_subject, nats, semantics_registry))]
pub async fn handle_list<H, N>(
    handler: &H,
    payload: &[u8],
    reply_subject: Option<String>,
    nats: &N,
    semantics_registry: &PushDeliverySemanticsRegistry,
) where
    H: A2aHandler,
    N: trogon_nats::PublishClient,
{
    let Some(reply) = reply_subject else {
        warn!("push_notification_list received without reply subject; dropping");
        return;
    };

    let (id, result) =
        unary_parse_and_call(handler, payload, |h, p| Box::pin(h.push_notification_list(p))).await;

    match result {
        Ok(resp) => {
            let Ok(mut envelope) = serde_json::to_value(resp) else {
                send_error(nats, &reply, id, A2aError::internal("failed to serialize list response")).await;
                return;
            };

            if let Some(serde_json::Value::Array(configs)) =
                envelope.as_object_mut().and_then(|m| m.get_mut("configs"))
            {
                for entry in configs.iter_mut() {
                    if let serde_json::Value::Object(cfg_obj) = entry {
                        let task_id_dom = cfg_obj
                            .get("taskId")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_owned();
                        inject_registry_semantics(semantics_registry, task_id_dom.as_str(), cfg_obj);
                    }
                }
            }

            reply_success(nats, &reply, id, envelope).await;
        }
        Err(e) => send_error(nats, &reply, id, e).await,
    }
}

#[instrument(
    name = "a2a.agent.push_notification_delete",
    skip(handler, payload, reply_subject, nats, semantics_registry)
)]
pub async fn handle_delete<H, N>(
    handler: &H,
    payload: &[u8],
    reply_subject: Option<String>,
    nats: &N,
    semantics_registry: &PushDeliverySemanticsRegistry,
) where
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

    match handler.push_notification_delete(params.clone()).await {
        Ok(()) => {
            if let (Ok(task_id_dom), Ok(cfg_dom)) = (
                A2aTaskId::new(params.task_id.clone()),
                PushNotificationConfigId::new(params.id.clone()),
            ) {
                semantics_registry.remove(&task_id_dom, &cfg_dom);
            }

            match JsonRpcResponse::new(id, serde_json::Value::Null).to_bytes() {
                Ok(b) => {
                    let headers = async_nats::HeaderMap::new();
                    if let Err(e) = nats
                        .publish_with_headers(async_nats::Subject::from(reply.as_str()), headers, Bytes::from(b))
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

async fn send_error<N: trogon_nats::PublishClient>(nats: &N, reply: &str, id: Option<JsonRpcId>, err: A2aError) {
    let bytes = JsonRpcErrorResponse::new(id, err.code, err.message).to_bytes();
    match bytes {
        Ok(b) => {
            let headers = async_nats::HeaderMap::new();
            if let Err(e) = nats
                .publish_with_headers(async_nats::Subject::from(reply), headers, Bytes::from(b))
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
    use crate::push::delivery_semantics::DeliverySemantics;
    use crate::task_id::A2aTaskId;
    use trogon_nats::AdvancedMockNatsClient;

    #[tokio::test]
    async fn set_success() {
        let nats = AdvancedMockNatsClient::new();
        let semantics = PushDeliverySemanticsRegistry::default();
        let handler = stub();
        handler.lock().unwrap().push_set_result = Some(Ok(a2a_types::TaskPushNotificationConfig {
            id: "cfg1".into(),
            task_id: "t1".into(),
            ..Default::default()
        }));
        handle_set(
            &handler,
            &rpc_payload("tasks/pushNotificationConfig/set", 1),
            Some("r".into()),
            &nats,
            &semantics,
        )
        .await;
        let body = parse_response(&nats.published_payloads()[0]);
        assert_eq!(body["result"]["id"], "cfg1");
        let tid = A2aTaskId::new("t1").unwrap();
        let cid = PushNotificationConfigId::new("cfg1").unwrap();
        assert_eq!(semantics.get(&tid, &cid), DeliverySemantics::AtLeastOnce);
    }

    #[tokio::test]
    async fn get_not_supported() {
        let nats = AdvancedMockNatsClient::new();
        let semantics = PushDeliverySemanticsRegistry::default();
        let handler = stub();
        handler.lock().unwrap().push_get_result = Some(Err(A2aError::push_notification_not_supported("no")));
        handle_get(
            &handler,
            &rpc_payload("tasks/pushNotificationConfig/get", 2),
            Some("r".into()),
            &nats,
            &semantics,
        )
        .await;
        let body = parse_response(&nats.published_payloads()[0]);
        assert_eq!(body["error"]["code"], crate::error::PUSH_NOTIFICATION_NOT_SUPPORTED);
    }

    #[tokio::test]
    async fn list_success() {
        let nats = AdvancedMockNatsClient::new();
        let semantics = PushDeliverySemanticsRegistry::default();
        let handler = stub();
        handler.lock().unwrap().push_list_result =
            Some(Ok(a2a_types::ListTaskPushNotificationConfigsResponse::default()));
        handle_list(
            &handler,
            &rpc_payload("tasks/pushNotificationConfig/list", 3),
            Some("r".into()),
            &nats,
            &semantics,
        )
        .await;
        let body = parse_response(&nats.published_payloads()[0]);
        assert!(body.get("result").is_some());
    }

    #[tokio::test]
    async fn delete_success_returns_null_result() {
        let nats = AdvancedMockNatsClient::new();
        let semantics = PushDeliverySemanticsRegistry::default();
        let handler = stub();
        handler.lock().unwrap().push_delete_result = Some(Ok(()));
        handle_delete(
            &handler,
            &rpc_payload("tasks/pushNotificationConfig/delete", 4),
            Some("r".into()),
            &nats,
            &semantics,
        )
        .await;
        let body = parse_response(&nats.published_payloads()[0]);
        assert_eq!(body["result"], serde_json::Value::Null);
    }

    #[tokio::test]
    async fn no_reply_drops_message() {
        let nats = AdvancedMockNatsClient::new();
        let semantics = PushDeliverySemanticsRegistry::default();
        let handler = stub();
        handle_set(
            &handler,
            &rpc_payload("tasks/pushNotificationConfig/set", 5),
            None,
            &nats,
            &semantics,
        )
        .await;
        assert!(nats.published_messages().is_empty());
    }
}
