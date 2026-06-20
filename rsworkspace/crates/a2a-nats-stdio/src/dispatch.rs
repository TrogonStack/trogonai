use a2a::event::StreamResponse;
use a2a::types::{
    CancelTaskRequest, DeleteTaskPushNotificationConfigRequest, GetTaskPushNotificationConfigRequest, GetTaskRequest,
    ListTaskPushNotificationConfigsRequest, ListTasksRequest, SendMessageRequest, TaskPushNotificationConfig,
};
use a2a_nats::client::{A2aClient, ClientError};
use a2a_nats::task_id::A2aTaskId;
use futures::StreamExt;
use serde_json::Value;
use tokio::sync::mpsc;
use trogon_nats::RequestClient;
use trogon_nats::jetstream::{JetStreamCreateConsumer, JetStreamGetStream, JsAck, JsMessageOf, JsMessageRef};

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
        ClientError::ExtendedAgentCardNotConfigured => {
            (a2a_nats::error::EXTENDED_AGENT_CARD_NOT_CONFIGURED, err.to_string())
        }
        ClientError::ExtensionSupportRequired(_) => (a2a_nats::error::EXTENSION_SUPPORT_REQUIRED, err.to_string()),
        ClientError::VersionNotSupported(_) => (a2a_nats::error::VERSION_NOT_SUPPORTED, err.to_string()),
        ClientError::AgentUnavailable => (a2a_nats::error::AGENT_UNAVAILABLE, err.to_string()),
        ClientError::JsonRpc { code, message } => (*code, message.clone()),
        _ => (-32603, err.to_string()),
    };
    OutboundFrame::Error(OutboundError::new(id, code, message))
}

fn parse_params<T: serde::de::DeserializeOwned>(params: Value) -> Result<T, OutboundFrame> {
    serde_json::from_value(params)
        .map_err(|e| OutboundFrame::Error(OutboundError::new(RpcId::Null, INVALID_PARAMS, e.to_string())))
}

/// `method` is the JSON-RPC method this notification is associated with —
/// `message/stream` for the streaming send path, `tasks/resubscribe` for the
/// resubscribe path. Clients route notifications by method, so emitting the
/// wrong one makes the resubscribe stream invisible to compliant callers.
fn stream_event_to_frame(id: &RpcId, method: &'static str, event: &StreamResponse) -> OutboundFrame {
    let params = serde_json::to_value(event).unwrap_or(Value::Null);
    OutboundFrame::Notification(OutboundNotification::new(id.clone(), method, params))
}

pub async fn dispatch_request<N, J>(
    client: &A2aClient<N, J>,
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
                                let frame = stream_event_to_frame(&id, "message/stream", &event);
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
            // Match the A2A wire shape used by sibling task RPCs and
            // `SubscribeToTaskRequest`: `id` is the task id; `lastSeq` is the
            // stdio-bridge-specific resume cursor.
            #[derive(serde::Deserialize)]
            #[serde(rename_all = "camelCase")]
            struct ResubParams {
                id: String,
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
            let task_id = match A2aTaskId::new(&p.id) {
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
                Ok((snapshot, mut stream)) => {
                    let result = serde_json::to_value(&snapshot).unwrap_or(Value::Null);
                    let bootstrap_frame = OutboundFrame::Response(OutboundResponse::new(id.clone(), result));
                    let _ = tx.send(bootstrap_frame).await;

                    while let Some(item) = stream.next().await {
                        match item {
                            Ok(event) => {
                                let frame = stream_event_to_frame(&id, "tasks/resubscribe", &event);
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
    use a2a_nats::client::A2aClient;
    use a2a_nats::{A2aAgentId, A2aPrefix};
    use bytes::Bytes;
    use serde_json::json;
    use trogon_nats::AdvancedMockNatsClient;
    use trogon_nats::jetstream::mocks::{MockJetStreamConsumer, MockJetStreamConsumerFactory};

    use crate::wire::RpcError;

    fn make_client(
        nats: AdvancedMockNatsClient,
        js: MockJetStreamConsumerFactory,
    ) -> A2aClient<AdvancedMockNatsClient, MockJetStreamConsumerFactory> {
        let prefix = A2aPrefix::new("a2a").unwrap();
        let agent_id = A2aAgentId::new("bot").unwrap();
        A2aClient::new(prefix, agent_id, nats, js)
    }

    fn task_response(task_id: &str) -> Bytes {
        let task = a2a::types::Task {
            id: task_id.to_string(),
            context_id: String::new(),
            status: a2a::types::TaskStatus {
                state: a2a::types::TaskState::Completed,
                message: None,
                timestamp: None,
            },
            artifacts: None,
            history: None,
            metadata: None,
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
        let task = a2a::types::Task {
            id: task_id.to_string(),
            context_id: String::new(),
            status: a2a::types::TaskStatus {
                state: a2a::types::TaskState::Unspecified,
                message: None,
                timestamp: None,
            },
            artifacts: None,
            history: None,
            metadata: None,
        };
        let response = a2a::types::SendMessageResponse::Task(task);
        serde_json::to_vec(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": "x",
            "result": response
        }))
        .unwrap()
        .into()
    }

    async fn dispatch(
        client: &A2aClient<AdvancedMockNatsClient, MockJetStreamConsumerFactory>,
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
        nats.set_response("a2a.agents.bot.tasks.get", task_response("t1"));
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
        nats.set_response("a2a.agents.bot.tasks.cancel", task_response("tc"));
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
        nats.set_response("a2a.agents.bot.message.send", send_message_response_bytes("ms"));
        let client = make_client(nats, MockJetStreamConsumerFactory::new());
        let frame = dispatch(
            &client,
            RpcId::Number(3),
            "message/send",
            json!({"message": {"messageId": "m1", "role": "ROLE_USER", "parts": []}}),
        )
        .await;
        assert!(
            matches!(frame, OutboundFrame::Response(_)),
            "got: {}",
            serde_json::to_string(&frame).unwrap()
        );
    }

    #[tokio::test]
    async fn message_stream_emits_bootstrap_then_events() {
        let nats = AdvancedMockNatsClient::new();
        nats.set_response("a2a.agents.bot.message.stream", send_message_response_bytes("ms2"));

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
    async fn tasks_resubscribe_emits_snapshot_then_empty_stream() {
        let nats = AdvancedMockNatsClient::new();
        nats.set_response("a2a.agents.bot.tasks.resubscribe", task_response("task1"));

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
            json!({"id": "task1", "lastSeq": 0}),
            &chan_tx,
        )
        .await;
        drop(chan_tx);

        let first = chan_rx.recv().await.expect("expected bootstrap frame");
        assert!(matches!(first, OutboundFrame::Response(_)));
        assert!(chan_rx.recv().await.is_none());
    }

    #[tokio::test]
    async fn agent_card_success() {
        let nats = AdvancedMockNatsClient::new();
        let card = a2a::agent_card::AgentCard {
            name: "BotA".into(),
            description: "desc".into(),
            version: "1.0".into(),
            capabilities: a2a::agent_card::AgentCapabilities::default(),
            supported_interfaces: vec![],
            default_input_modes: vec![],
            default_output_modes: vec![],
            skills: vec![],
            provider: None,
            documentation_url: None,
            icon_url: None,
            security_schemes: None,
            security_requirements: None,
            signatures: None,
        };
        nats.set_response(
            "a2a.agents.bot.card",
            serde_json::to_vec(&serde_json::json!({
                "jsonrpc": "2.0", "id": "x", "result": card
            }))
            .unwrap()
            .into(),
        );
        let client = make_client(nats, MockJetStreamConsumerFactory::new());
        let frame = dispatch(
            &client,
            RpcId::Number(6),
            "agent/getAuthenticatedExtendedCard",
            json!({}),
        )
        .await;
        assert!(matches!(frame, OutboundFrame::Response(_)));
    }

    #[tokio::test]
    async fn unknown_method_returns_method_not_found() {
        let nats = AdvancedMockNatsClient::new();
        let client = make_client(nats, MockJetStreamConsumerFactory::new());
        let frame = dispatch(&client, RpcId::Number(7), "bogus/method", json!({})).await;
        assert!(matches!(
            frame,
            OutboundFrame::Error(OutboundError {
                error: RpcError { code: -32601, .. },
                ..
            })
        ));
    }

    #[tokio::test]
    async fn invalid_params_returns_error() {
        let nats = AdvancedMockNatsClient::new();
        let client = make_client(nats, MockJetStreamConsumerFactory::new());
        let frame = dispatch(&client, RpcId::Number(8), "tasks/get", json!("not an object")).await;
        assert!(matches!(
            frame,
            OutboundFrame::Error(OutboundError {
                error: RpcError { code: -32602, .. },
                ..
            })
        ));
    }

    fn err_response(code: i32, msg: &str) -> Bytes {
        let json = serde_json::json!({"jsonrpc": "2.0", "id": "x", "error": {"code": code, "message": msg}});
        serde_json::to_vec(&json).unwrap().into()
    }

    #[track_caller]
    fn assert_err_code(frame: OutboundFrame, expected: i32) {
        let OutboundFrame::Error(OutboundError {
            error: RpcError { code, .. },
            ..
        }) = frame
        else {
            panic!("expected error frame, got non-error variant");
        };
        assert_eq!(code, expected);
    }

    #[tokio::test]
    async fn tasks_list_success() {
        let nats = AdvancedMockNatsClient::new();
        let list = a2a::types::ListTasksResponse {
            tasks: vec![],
            next_page_token: String::new(),
            page_size: 0,
            total_size: 0,
        };
        let body = serde_json::json!({"jsonrpc":"2.0","id":"x","result":list});
        nats.set_response("a2a.agents.bot.tasks.list", serde_json::to_vec(&body).unwrap().into());
        let client = make_client(nats, MockJetStreamConsumerFactory::new());
        let frame = dispatch(&client, RpcId::Number(1), "tasks/list", json!({})).await;
        assert!(matches!(frame, OutboundFrame::Response(_)));
    }

    #[tokio::test]
    async fn push_set_success() {
        let nats = AdvancedMockNatsClient::new();
        let cfg = a2a::types::TaskPushNotificationConfig {
            url: "https://example.com".into(),
            id: Some("c".into()),
            task_id: "t1".into(),
            token: None,
            authentication: None,
            tenant: None,
        };
        let body = serde_json::json!({"jsonrpc":"2.0","id":"x","result":cfg});
        nats.set_response("a2a.agents.bot.push.set", serde_json::to_vec(&body).unwrap().into());
        let client = make_client(nats, MockJetStreamConsumerFactory::new());
        let frame = dispatch(
            &client,
            RpcId::Number(2),
            "tasks/pushNotificationConfig/set",
            json!({"url":"https://example.com","id":"c","taskId":"t1"}),
        )
        .await;
        assert!(matches!(frame, OutboundFrame::Response(_)));
    }

    #[tokio::test]
    async fn push_get_success() {
        let nats = AdvancedMockNatsClient::new();
        let cfg = a2a::types::TaskPushNotificationConfig {
            url: "https://example.com".into(),
            id: Some("c".into()),
            task_id: "t1".into(),
            token: None,
            authentication: None,
            tenant: None,
        };
        let body = serde_json::json!({"jsonrpc":"2.0","id":"x","result":cfg});
        nats.set_response("a2a.agents.bot.push.get", serde_json::to_vec(&body).unwrap().into());
        let client = make_client(nats, MockJetStreamConsumerFactory::new());
        let frame = dispatch(
            &client,
            RpcId::Number(3),
            "tasks/pushNotificationConfig/get",
            json!({"taskId":"t1","id":"c"}),
        )
        .await;
        assert!(matches!(frame, OutboundFrame::Response(_)));
    }

    #[tokio::test]
    async fn push_list_success() {
        let nats = AdvancedMockNatsClient::new();
        let resp = a2a::types::ListTaskPushNotificationConfigsResponse {
            configs: vec![],
            next_page_token: None,
        };
        let body = serde_json::json!({"jsonrpc":"2.0","id":"x","result":resp});
        nats.set_response("a2a.agents.bot.push.list", serde_json::to_vec(&body).unwrap().into());
        let client = make_client(nats, MockJetStreamConsumerFactory::new());
        let frame = dispatch(
            &client,
            RpcId::Number(4),
            "tasks/pushNotificationConfig/list",
            json!({"taskId":"t1"}),
        )
        .await;
        assert!(matches!(frame, OutboundFrame::Response(_)));
    }

    #[tokio::test]
    async fn push_delete_success() {
        let nats = AdvancedMockNatsClient::new();
        let body = serde_json::json!({"jsonrpc":"2.0","id":"x","result":null});
        nats.set_response("a2a.agents.bot.push.delete", serde_json::to_vec(&body).unwrap().into());
        let client = make_client(nats, MockJetStreamConsumerFactory::new());
        let frame = dispatch(
            &client,
            RpcId::Number(5),
            "tasks/pushNotificationConfig/delete",
            json!({"taskId":"t1","id":"c"}),
        )
        .await;
        assert!(matches!(frame, OutboundFrame::Response(_)));
    }

    #[tokio::test]
    async fn client_err_to_frame_maps_every_typed_variant() {
        // Each entry maps a ClientError variant produced by the agent's JSON-RPC error code
        // to the expected outbound error code.
        let cases = [
            (a2a_nats::error::TASK_NOT_FOUND, a2a_nats::error::TASK_NOT_FOUND),
            (
                a2a_nats::error::TASK_NOT_CANCELABLE,
                a2a_nats::error::TASK_NOT_CANCELABLE,
            ),
            (
                a2a_nats::error::PUSH_NOTIFICATION_NOT_SUPPORTED,
                a2a_nats::error::PUSH_NOTIFICATION_NOT_SUPPORTED,
            ),
            (
                a2a_nats::error::UNSUPPORTED_OPERATION,
                a2a_nats::error::UNSUPPORTED_OPERATION,
            ),
            (
                a2a_nats::error::CONTENT_TYPE_NOT_SUPPORTED,
                a2a_nats::error::CONTENT_TYPE_NOT_SUPPORTED,
            ),
            (
                a2a_nats::error::INVALID_AGENT_RESPONSE,
                a2a_nats::error::INVALID_AGENT_RESPONSE,
            ),
            (
                a2a_nats::error::EXTENDED_AGENT_CARD_NOT_CONFIGURED,
                a2a_nats::error::EXTENDED_AGENT_CARD_NOT_CONFIGURED,
            ),
            (
                a2a_nats::error::EXTENSION_SUPPORT_REQUIRED,
                a2a_nats::error::EXTENSION_SUPPORT_REQUIRED,
            ),
            (
                a2a_nats::error::VERSION_NOT_SUPPORTED,
                a2a_nats::error::VERSION_NOT_SUPPORTED,
            ),
            (a2a_nats::error::AGENT_UNAVAILABLE, a2a_nats::error::AGENT_UNAVAILABLE),
            (-32099, -32099), // generic JSON-RPC fallthrough preserves the upstream code
        ];
        for (input, expected) in cases {
            let nats = AdvancedMockNatsClient::new();
            nats.set_response("a2a.agents.bot.tasks.get", err_response(input, "x"));
            let client = make_client(nats, MockJetStreamConsumerFactory::new());
            let frame = dispatch(
                &client,
                RpcId::Number(input as i64),
                "tasks/get",
                json!({"id":"t","tenant":""}),
            )
            .await;
            assert_err_code(frame, expected);
        }
    }

    #[tokio::test]
    async fn transport_error_falls_back_to_internal_code() {
        let nats = AdvancedMockNatsClient::new();
        nats.fail_next_request();
        let client = make_client(nats, MockJetStreamConsumerFactory::new());
        let frame = dispatch(&client, RpcId::Number(99), "tasks/get", json!({"id":"t","tenant":""})).await;
        assert_err_code(frame, -32603);
    }

    #[tokio::test]
    async fn agent_error_routes_to_outbound_error_for_every_typed_method() {
        // For each method, configure the agent to reply with a typed JSON-RPC
        // error and confirm the dispatcher forwards it as an OutboundError
        // through that method's Err arm.
        let cases = [
            (
                "a2a.agents.bot.message.send",
                "message/send",
                json!({"message": {"messageId": "m", "role": "ROLE_USER", "parts": []}}),
            ),
            ("a2a.agents.bot.tasks.list", "tasks/list", json!({})),
            (
                "a2a.agents.bot.tasks.cancel",
                "tasks/cancel",
                json!({"id": "t", "tenant": ""}),
            ),
            (
                "a2a.agents.bot.push.set",
                "tasks/pushNotificationConfig/set",
                json!({"url":"https://example.com","id":"c","taskId":"t1"}),
            ),
            (
                "a2a.agents.bot.push.get",
                "tasks/pushNotificationConfig/get",
                json!({"taskId":"t1","id":"c"}),
            ),
            (
                "a2a.agents.bot.push.list",
                "tasks/pushNotificationConfig/list",
                json!({"taskId":"t1"}),
            ),
            (
                "a2a.agents.bot.push.delete",
                "tasks/pushNotificationConfig/delete",
                json!({"taskId":"t1","id":"c"}),
            ),
            ("a2a.agents.bot.card", "agent/getAuthenticatedExtendedCard", json!({})),
        ];
        for (subject, method, params) in cases {
            let nats = AdvancedMockNatsClient::new();
            nats.set_response(subject, err_response(a2a_nats::error::TASK_NOT_FOUND, "missing"));
            let client = make_client(nats, MockJetStreamConsumerFactory::new());
            let frame = dispatch(&client, RpcId::Number(1), method, params).await;
            assert_err_code(frame, a2a_nats::error::TASK_NOT_FOUND);
        }
    }

    #[tokio::test]
    async fn message_stream_error_at_bootstrap_routes_to_outbound_error() {
        let nats = AdvancedMockNatsClient::new();
        nats.set_response(
            "a2a.agents.bot.message.stream",
            err_response(a2a_nats::error::AGENT_UNAVAILABLE, "down"),
        );
        let js = MockJetStreamConsumerFactory::new();
        let (consumer, _tx) = MockJetStreamConsumer::new();
        js.add_consumer(consumer);
        let client = make_client(nats, js);
        let frame = dispatch(
            &client,
            RpcId::Number(1),
            "message/stream",
            json!({"message": {"messageId": "m", "role": "ROLE_USER", "parts": []}}),
        )
        .await;
        assert_err_code(frame, a2a_nats::error::AGENT_UNAVAILABLE);
    }

    #[tokio::test]
    async fn tasks_resubscribe_error_at_snapshot_routes_to_outbound_error() {
        let nats = AdvancedMockNatsClient::new();
        nats.set_response(
            "a2a.agents.bot.tasks.resubscribe",
            err_response(a2a_nats::error::TASK_NOT_FOUND, "gone"),
        );
        let js = MockJetStreamConsumerFactory::new();
        let (consumer, _tx) = MockJetStreamConsumer::new();
        js.add_consumer(consumer);
        let client = make_client(nats, js);
        let frame = dispatch(
            &client,
            RpcId::Number(1),
            "tasks/resubscribe",
            json!({"id": "missing", "lastSeq": 0}),
        )
        .await;
        assert_err_code(frame, a2a_nats::error::TASK_NOT_FOUND);
    }

    #[tokio::test]
    async fn invalid_params_returned_for_every_typed_method() {
        let nats = AdvancedMockNatsClient::new();
        let client = make_client(nats, MockJetStreamConsumerFactory::new());
        for method in [
            "message/send",
            "message/stream",
            "tasks/list",
            "tasks/cancel",
            "tasks/resubscribe",
            "tasks/pushNotificationConfig/set",
            "tasks/pushNotificationConfig/get",
            "tasks/pushNotificationConfig/list",
            "tasks/pushNotificationConfig/delete",
        ] {
            let frame = dispatch(&client, RpcId::Number(1), method, json!("not an object")).await;
            assert_err_code(frame, -32602);
        }
    }

    use trogon_nats::jetstream::mocks::MockJsMessage;

    fn js_msg(payload: Vec<u8>) -> MockJsMessage {
        let inner = async_nats::Message {
            subject: "a2a.tasks.task1.events.req".into(),
            reply: Some("$JS.ACK.A2A_EVENTS.consumer.1.1.1.0.0".into()),
            payload: Bytes::from(payload),
            headers: None,
            status: None,
            description: None,
            length: 0,
        };
        MockJsMessage::new(inner)
    }

    fn status_event(task_id: &str) -> a2a::event::StreamResponse {
        a2a::event::StreamResponse::StatusUpdate(a2a::event::TaskStatusUpdateEvent {
            task_id: task_id.to_string(),
            context_id: "ctx".into(),
            status: a2a::types::TaskStatus {
                state: a2a::types::TaskState::Working,
                message: None,
                timestamp: None,
            },
            metadata: None,
        })
    }

    #[tokio::test]
    async fn message_stream_forwards_status_events_as_notifications() {
        let nats = AdvancedMockNatsClient::new();
        nats.set_response("a2a.agents.bot.message.stream", send_message_response_bytes("ms3"));
        let js = MockJetStreamConsumerFactory::new();
        let (consumer, evt_tx) = MockJetStreamConsumer::new();
        js.add_consumer(consumer);
        let event_payload = serde_json::to_vec(&status_event("task-stream")).unwrap();
        evt_tx.unbounded_send(Ok(js_msg(event_payload))).unwrap();
        drop(evt_tx);
        let client = make_client(nats, js);
        let (chan_tx, mut chan_rx) = mpsc::channel(16);
        dispatch_request(
            &client,
            RpcId::Number(10),
            "message/stream",
            json!({"message": {"messageId": "m3", "role": "ROLE_USER", "parts": []}}),
            &chan_tx,
        )
        .await;
        drop(chan_tx);
        let first = chan_rx.recv().await.expect("bootstrap");
        assert!(matches!(first, OutboundFrame::Response(_)));
        let second = chan_rx.recv().await.expect("event notification");
        match second {
            OutboundFrame::Notification(n) => assert_eq!(n.method, "message/stream"),
            other => panic!("expected notification, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn tasks_resubscribe_forwards_status_events_under_resubscribe_method() {
        let nats = AdvancedMockNatsClient::new();
        nats.set_response("a2a.agents.bot.tasks.resubscribe", task_response("rsub"));
        let js = MockJetStreamConsumerFactory::new();
        let (consumer, evt_tx) = MockJetStreamConsumer::new();
        js.add_consumer(consumer);
        let event_payload = serde_json::to_vec(&status_event("rsub")).unwrap();
        evt_tx.unbounded_send(Ok(js_msg(event_payload))).unwrap();
        drop(evt_tx);
        let client = make_client(nats, js);
        let (chan_tx, mut chan_rx) = mpsc::channel(16);
        dispatch_request(
            &client,
            RpcId::Number(11),
            "tasks/resubscribe",
            json!({"id": "rsub", "lastSeq": 0}),
            &chan_tx,
        )
        .await;
        drop(chan_tx);
        let first = chan_rx.recv().await.expect("snapshot");
        assert!(matches!(first, OutboundFrame::Response(_)));
        let second = chan_rx.recv().await.expect("event notification");
        match second {
            // Notification method MUST be tasks/resubscribe, not message/stream.
            OutboundFrame::Notification(n) => assert_eq!(n.method, "tasks/resubscribe"),
            other => panic!("expected notification, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn tasks_resubscribe_rejects_blank_id() {
        let nats = AdvancedMockNatsClient::new();
        let client = make_client(nats, MockJetStreamConsumerFactory::new());
        let frame = dispatch(
            &client,
            RpcId::Number(11),
            "tasks/resubscribe",
            json!({"id": "", "lastSeq": 0}),
        )
        .await;
        assert_err_code(frame, -32602);
    }

    #[test]
    fn make_with_id_overwrites_error_id_and_passes_through_non_error_frames() {
        let from_parse_helper = OutboundFrame::Error(OutboundError::new(RpcId::Null, INVALID_PARAMS, "x".into()));
        let target_id = RpcId::Number(42);
        if let OutboundFrame::Error(e) = super::make_with_id(from_parse_helper, &target_id) {
            assert_eq!(e.id, RpcId::Number(42));
        }
        // Non-Error variants pass through unchanged — there's nothing to rewrite.
        let notif = OutboundFrame::Notification(OutboundNotification::new(
            RpcId::Number(1),
            "message/stream",
            Value::Null,
        ));
        if let OutboundFrame::Notification(n) = super::make_with_id(notif, &target_id) {
            assert_eq!(n.id, RpcId::Number(1));
        }
    }
}
