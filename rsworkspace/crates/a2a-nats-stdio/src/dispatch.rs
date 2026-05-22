use a2a_nats::client::{
    CancelTaskRequest, Client, ClientError, DeleteTaskPushNotificationConfigRequest,
    GetTaskPushNotificationConfigRequest, GetTaskRequest, ListTaskPushNotificationConfigsRequest, ListTasksRequest,
    SendMessageRequest, TaskPushNotificationConfig,
};
use a2a_nats::task_id::A2aTaskId;
use a2a_types::StreamResponse;
use futures::StreamExt;
use serde_json::Value;
use tokio::sync::mpsc;
use trogon_nats::jetstream::{JetStreamCreateConsumer, JetStreamGetStream, JsAck, JsMessageOf, JsMessageRef};
use trogon_nats::RequestClient;

use crate::wire::{OutboundError, OutboundFrame, OutboundNotification, OutboundResponse, RpcId};

const METHOD_NOT_FOUND: i32 = -32601;
const INVALID_PARAMS: i32 = -32602;

fn client_err_to_frame(id: RpcId, err: ClientError) -> OutboundFrame {
    let (code, message) = match &err {
        ClientError::TaskNotFound => (a2a_nats::error::TASK_NOT_FOUND, err.to_string()),
        ClientError::TaskNotCancelable => (a2a_nats::error::TASK_NOT_CANCELABLE, err.to_string()),
        ClientError::PushNotificationNotSupported => {
            (a2a_nats::error::PUSH_NOTIFICATION_NOT_SUPPORTED, err.to_string())
        }
        ClientError::UnsupportedOperation => (a2a_nats::error::UNSUPPORTED_OPERATION, err.to_string()),
        ClientError::ContentTypeNotSupported => (a2a_nats::error::CONTENT_TYPE_NOT_SUPPORTED, err.to_string()),
        ClientError::InvalidAgentResponse => (a2a_nats::error::INVALID_AGENT_RESPONSE, err.to_string()),
        ClientError::AgentUnavailable => (a2a_nats::error::AGENT_UNAVAILABLE, err.to_string()),
        ClientError::JsonRpc { code, message } => (*code, message.clone()),
        _ => (-32603, err.to_string()),
    };
    OutboundFrame::Error(OutboundError::new(id, code, message))
}

fn parse_params<T: serde::de::DeserializeOwned>(params: Value) -> Result<T, OutboundFrame> {
    serde_json::from_value(params).map_err(|e| {
        OutboundFrame::Error(OutboundError::new(RpcId::Null, INVALID_PARAMS, e.to_string()))
    })
}

fn stream_event_to_frame(id: &RpcId, event: &StreamResponse) -> OutboundFrame {
    let params = serde_json::to_value(event).unwrap_or(Value::Null);
    OutboundFrame::Notification(OutboundNotification::new(id.clone(), "message/stream", params))
}

pub async fn dispatch_request<N, J>(
    client: &Client<N, J>,
    id: RpcId,
    method: &str,
    params: Value,
    tx: &mpsc::Sender<OutboundFrame>,
) where
    N: RequestClient,
    J: JetStreamGetStream,
    JsMessageOf<J>: JsMessageRef + JsAck<Error: std::fmt::Display + Send + 'static> + Send + 'static,
    <J as JetStreamGetStream>::Stream: Send + 'static,
    <<J as JetStreamGetStream>::Stream as JetStreamCreateConsumer>::Consumer: Send + 'static,
    <<<J as JetStreamGetStream>::Stream as JetStreamCreateConsumer>::Consumer as trogon_nats::jetstream::JetStreamConsumer>::Messages: Send + 'static,
    <<<J as JetStreamGetStream>::Stream as JetStreamCreateConsumer>::Consumer as trogon_nats::jetstream::JetStreamConsumer>::MessagesError: std::fmt::Display + Send + 'static,
    <<<J as JetStreamGetStream>::Stream as JetStreamCreateConsumer>::Consumer as trogon_nats::jetstream::JetStreamConsumer>::StreamError: std::fmt::Display + Send + 'static,
{
    let frame = match method {
        "message/send" => {
            let req: SendMessageRequest = match parse_params(params) {
                Ok(r) => r,
                Err(f) => {
                    let _ = tx.send(make_with_id(f, &id)).await;
                    return;
                }
            };
            match client.message_send(&req).await {
                Ok(resp) => {
                    let result = serde_json::to_value(&resp).unwrap_or(Value::Null);
                    OutboundFrame::Response(OutboundResponse::new(id, result))
                }
                Err(e) => client_err_to_frame(id, e),
            }
        }

        "message/stream" => {
            let req: SendMessageRequest = match parse_params(params) {
                Ok(r) => r,
                Err(f) => {
                    let _ = tx.send(make_with_id(f, &id)).await;
                    return;
                }
            };
            match client.message_stream(&req).await {
                Err(e) => client_err_to_frame(id, e),
                Ok((bootstrap, mut stream)) => {
                    let result = serde_json::to_value(&bootstrap).unwrap_or(Value::Null);
                    let bootstrap_frame = OutboundFrame::Response(OutboundResponse::new(id.clone(), result));
                    let _ = tx.send(bootstrap_frame).await;

                    while let Some(item) = stream.next().await {
                        match item {
                            Ok(event) => {
                                let frame = stream_event_to_frame(&id, &event);
                                if tx.send(frame).await.is_err() {
                                    return;
                                }
                            }
                            Err(e) => {
                                let _ = tx.send(client_err_to_frame(id, e)).await;
                                return;
                            }
                        }
                    }
                    return;
                }
            }
        }

        "tasks/get" => {
            let req: GetTaskRequest = match parse_params(params) {
                Ok(r) => r,
                Err(f) => {
                    let _ = tx.send(make_with_id(f, &id)).await;
                    return;
                }
            };
            match client.tasks_get(&req).await {
                Ok(task) => {
                    let result = serde_json::to_value(&task).unwrap_or(Value::Null);
                    OutboundFrame::Response(OutboundResponse::new(id, result))
                }
                Err(e) => client_err_to_frame(id, e),
            }
        }

        "tasks/list" => {
            let req: ListTasksRequest = match parse_params(params) {
                Ok(r) => r,
                Err(f) => {
                    let _ = tx.send(make_with_id(f, &id)).await;
                    return;
                }
            };
            match client.tasks_list(&req).await {
                Ok(resp) => {
                    let result = serde_json::to_value(&resp).unwrap_or(Value::Null);
                    OutboundFrame::Response(OutboundResponse::new(id, result))
                }
                Err(e) => client_err_to_frame(id, e),
            }
        }

        "tasks/cancel" => {
            let req: CancelTaskRequest = match parse_params(params) {
                Ok(r) => r,
                Err(f) => {
                    let _ = tx.send(make_with_id(f, &id)).await;
                    return;
                }
            };
            match client.tasks_cancel(&req).await {
                Ok(task) => {
                    let result = serde_json::to_value(&task).unwrap_or(Value::Null);
                    OutboundFrame::Response(OutboundResponse::new(id, result))
                }
                Err(e) => client_err_to_frame(id, e),
            }
        }

        "tasks/resubscribe" => {
            #[derive(serde::Deserialize)]
            struct ResubParams {
                task_id: String,
                #[serde(default)]
                last_seq: u64,
            }
            let p: ResubParams = match parse_params(params) {
                Ok(r) => r,
                Err(f) => {
                    let _ = tx.send(make_with_id(f, &id)).await;
                    return;
                }
            };
            let task_id = match A2aTaskId::new(&p.task_id) {
                Ok(t) => t,
                Err(e) => {
                    let _ = tx
                        .send(OutboundFrame::Error(OutboundError::new(
                            id,
                            INVALID_PARAMS,
                            e.to_string(),
                        )))
                        .await;
                    return;
                }
            };
            match client.tasks_resubscribe(&task_id, p.last_seq).await {
                Err(e) => client_err_to_frame(id, e),
                Ok(mut stream) => {
                    while let Some(item) = stream.next().await {
                        match item {
                            Ok(event) => {
                                let frame = stream_event_to_frame(&id, &event);
                                if tx.send(frame).await.is_err() {
                                    return;
                                }
                            }
                            Err(e) => {
                                let _ = tx.send(client_err_to_frame(id, e)).await;
                                return;
                            }
                        }
                    }
                    return;
                }
            }
        }

        "tasks/pushNotificationConfig/set" => {
            let req: TaskPushNotificationConfig = match parse_params(params) {
                Ok(r) => r,
                Err(f) => {
                    let _ = tx.send(make_with_id(f, &id)).await;
                    return;
                }
            };
            match client.push_set(&req).await {
                Ok(cfg) => {
                    let result = serde_json::to_value(&cfg).unwrap_or(Value::Null);
                    OutboundFrame::Response(OutboundResponse::new(id, result))
                }
                Err(e) => client_err_to_frame(id, e),
            }
        }

        "tasks/pushNotificationConfig/get" => {
            let req: GetTaskPushNotificationConfigRequest = match parse_params(params) {
                Ok(r) => r,
                Err(f) => {
                    let _ = tx.send(make_with_id(f, &id)).await;
                    return;
                }
            };
            match client.push_get(&req).await {
                Ok(cfg) => {
                    let result = serde_json::to_value(&cfg).unwrap_or(Value::Null);
                    OutboundFrame::Response(OutboundResponse::new(id, result))
                }
                Err(e) => client_err_to_frame(id, e),
            }
        }

        "tasks/pushNotificationConfig/list" => {
            let req: ListTaskPushNotificationConfigsRequest = match parse_params(params) {
                Ok(r) => r,
                Err(f) => {
                    let _ = tx.send(make_with_id(f, &id)).await;
                    return;
                }
            };
            match client.push_list(&req).await {
                Ok(resp) => {
                    let result = serde_json::to_value(&resp).unwrap_or(Value::Null);
                    OutboundFrame::Response(OutboundResponse::new(id, result))
                }
                Err(e) => client_err_to_frame(id, e),
            }
        }

        "tasks/pushNotificationConfig/delete" => {
            let req: DeleteTaskPushNotificationConfigRequest = match parse_params(params) {
                Ok(r) => r,
                Err(f) => {
                    let _ = tx.send(make_with_id(f, &id)).await;
                    return;
                }
            };
            match client.push_delete(&req).await {
                Ok(()) => OutboundFrame::Response(OutboundResponse::new(id, Value::Null)),
                Err(e) => client_err_to_frame(id, e),
            }
        }

        "agent/getAuthenticatedExtendedCard" => match client.agent_card().await {
            Ok(card) => {
                let result = serde_json::to_value(&card).unwrap_or(Value::Null);
                OutboundFrame::Response(OutboundResponse::new(id, result))
            }
            Err(e) => client_err_to_frame(id, e),
        },

        unknown => OutboundFrame::Error(OutboundError::new(
            id,
            METHOD_NOT_FOUND,
            format!("method not found: {unknown}"),
        )),
    };

    let _ = tx.send(frame).await;
}

fn make_with_id(frame: OutboundFrame, id: &RpcId) -> OutboundFrame {
    match frame {
        OutboundFrame::Error(mut e) => {
            e.id = id.clone();
            OutboundFrame::Error(e)
        }
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use a2a_nats::client::Client;
    use a2a_nats::{A2aAgentId, A2aPrefix, Config, NatsConfig};
    use bytes::Bytes;
    use serde_json::json;
    use trogon_nats::AdvancedMockNatsClient;
    use trogon_nats::jetstream::mocks::{MockJetStreamConsumer, MockJetStreamConsumerFactory};

    use crate::wire::RpcError;

    fn test_config() -> Config {
        Config::new(
            A2aPrefix::new("a2a".to_string()).unwrap(),
            NatsConfig { servers: vec!["localhost:4222".to_string()], auth: trogon_nats::NatsAuth::None },
        )
    }

    fn make_client(
        nats: AdvancedMockNatsClient,
        js: MockJetStreamConsumerFactory,
    ) -> Client<AdvancedMockNatsClient, MockJetStreamConsumerFactory> {
        let agent_id = A2aAgentId::new("bot").unwrap();
        Client::new(test_config(), agent_id, nats, js)
    }

    fn task_response(task_id: &str) -> Bytes {
        let task = a2a_types::Task {
            id: task_id.to_string(),
            status: Some(a2a_types::TaskStatus {
                state: a2a_types::TaskState::Completed.into(),
                message: None,
                timestamp: None,
            }),
            ..Default::default()
        };
        serde_json::to_vec(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": "x",
            "result": task
        }))
        .unwrap()
        .into()
    }

    fn send_message_response_bytes(task_id: &str) -> Bytes {
        let task = a2a_types::Task {
            id: task_id.to_string(),
            ..Default::default()
        };
        let response = a2a_types::SendMessageResponse {
            payload: Some(a2a_types::send_message_response::Payload::Task(task)),
        };
        serde_json::to_vec(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": "x",
            "result": response
        }))
        .unwrap()
        .into()
    }

    async fn dispatch(
        client: &Client<AdvancedMockNatsClient, MockJetStreamConsumerFactory>,
        id: RpcId,
        method: &str,
        params: Value,
    ) -> OutboundFrame {
        let (tx, mut rx) = mpsc::channel(8);
        dispatch_request(client, id, method, params, &tx).await;
        drop(tx);
        rx.recv().await.expect("expected at least one frame")
    }

    #[tokio::test]
    async fn tasks_get_success() {
        let nats = AdvancedMockNatsClient::new();
        nats.set_response("a2a.agent.bot.tasks.get", task_response("t1"));
        let client = make_client(nats, MockJetStreamConsumerFactory::new());
        let frame = dispatch(
            &client,
            RpcId::Number(1),
            "tasks/get",
            json!({"id": "t1", "tenant": ""}),
        )
        .await;
        assert!(matches!(frame, OutboundFrame::Response(_)));
    }

    #[tokio::test]
    async fn tasks_get_error_maps_to_rpc_error() {
        let nats = AdvancedMockNatsClient::new();
        nats.fail_next_request();
        let client = make_client(nats, MockJetStreamConsumerFactory::new());
        let frame = dispatch(
            &client,
            RpcId::Number(2),
            "tasks/get",
            json!({"id": "t1", "tenant": ""}),
        )
        .await;
        assert!(matches!(frame, OutboundFrame::Error(_)));
    }

    #[tokio::test]
    async fn tasks_cancel_success() {
        let nats = AdvancedMockNatsClient::new();
        nats.set_response("a2a.agent.bot.tasks.cancel", task_response("tc"));
        let client = make_client(nats, MockJetStreamConsumerFactory::new());
        let frame = dispatch(
            &client,
            RpcId::String("x".into()),
            "tasks/cancel",
            json!({"id": "tc", "tenant": ""}),
        )
        .await;
        assert!(matches!(frame, OutboundFrame::Response(_)));
    }

    #[tokio::test]
    async fn message_send_success() {
        let nats = AdvancedMockNatsClient::new();
        nats.set_response("a2a.agent.bot.message.send", send_message_response_bytes("ms"));
        let client = make_client(nats, MockJetStreamConsumerFactory::new());
        let frame = dispatch(
            &client,
            RpcId::Number(3),
            "message/send",
            json!({"message": {"messageId": "m1", "role": "ROLE_USER", "parts": []}}),
        )
        .await;
        assert!(matches!(frame, OutboundFrame::Response(_)), "got: {}", serde_json::to_string(&frame).unwrap());
    }

    #[tokio::test]
    async fn message_stream_emits_bootstrap_then_events() {
        let nats = AdvancedMockNatsClient::new();
        nats.set_response("a2a.agent.bot.message.stream", send_message_response_bytes("ms2"));

        let js = MockJetStreamConsumerFactory::new();
        let (consumer, tx) = MockJetStreamConsumer::new();
        js.add_consumer(consumer);
        drop(tx);

        let client = make_client(nats, js);
        let (chan_tx, mut chan_rx) = mpsc::channel(16);
        dispatch_request(
            &client,
            RpcId::Number(4),
            "message/stream",
            json!({"message": {"messageId": "m2", "role": "ROLE_USER", "parts": []}}),
            &chan_tx,
        )
        .await;
        drop(chan_tx);

        let first = chan_rx.recv().await.expect("bootstrap frame");
        assert!(matches!(first, OutboundFrame::Response(_)));
    }

    #[tokio::test]
    async fn tasks_resubscribe_empty_stream() {
        let nats = AdvancedMockNatsClient::new();
        let js = MockJetStreamConsumerFactory::new();
        let (consumer, tx) = MockJetStreamConsumer::new();
        js.add_consumer(consumer);
        drop(tx);

        let client = make_client(nats, js);
        let (chan_tx, mut chan_rx) = mpsc::channel(8);
        dispatch_request(
            &client,
            RpcId::Number(5),
            "tasks/resubscribe",
            json!({"task_id": "task1", "last_seq": 0}),
            &chan_tx,
        )
        .await;
        drop(chan_tx);
        assert!(chan_rx.recv().await.is_none());
    }

    #[tokio::test]
    async fn agent_card_success() {
        let nats = AdvancedMockNatsClient::new();
        let card = a2a_types::AgentCard {
            name: "BotA".into(),
            description: "desc".into(),
            version: "1.0".into(),
            capabilities: Some(a2a_types::AgentCapabilities::default()),
            ..Default::default()
        };
        nats.set_response(
            "a2a.agent.bot.agent.card",
            serde_json::to_vec(&serde_json::json!({
                "jsonrpc": "2.0", "id": "x", "result": card
            }))
            .unwrap()
            .into(),
        );
        let client = make_client(nats, MockJetStreamConsumerFactory::new());
        let frame = dispatch(&client, RpcId::Number(6), "agent/getAuthenticatedExtendedCard", json!({})).await;
        assert!(matches!(frame, OutboundFrame::Response(_)));
    }

    #[tokio::test]
    async fn unknown_method_returns_method_not_found() {
        let nats = AdvancedMockNatsClient::new();
        let client = make_client(nats, MockJetStreamConsumerFactory::new());
        let frame = dispatch(&client, RpcId::Number(7), "bogus/method", json!({})).await;
        assert!(matches!(frame, OutboundFrame::Error(OutboundError { error: RpcError { code: -32601, .. }, .. })));
    }

    #[tokio::test]
    async fn invalid_params_returns_error() {
        let nats = AdvancedMockNatsClient::new();
        let client = make_client(nats, MockJetStreamConsumerFactory::new());
        let frame = dispatch(&client, RpcId::Number(8), "tasks/get", json!("not an object")).await;
        assert!(matches!(frame, OutboundFrame::Error(OutboundError { error: RpcError { code: -32602, .. }, .. })));
    }
}
