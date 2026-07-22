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

fn parse_params<T: serde::de::DeserializeOwned>(params: Value) -> Result<T, Box<OutboundFrame>> {
    serde_json::from_value(params).map_err(|e| {
        Box::new(OutboundFrame::Error(OutboundError::new(
            RpcId::Null,
            INVALID_PARAMS,
            e.to_string(),
        )))
    })
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
                    let _ = tx.send(make_with_id(*f, &id)).await;
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
                    let _ = tx.send(make_with_id(*f, &id)).await;
                    return;
                }
            };
            match client.message_stream(&req).await {
                Err(e) => client_err_to_frame(id, e),
                Ok((bootstrap, mut stream)) => {
                    let result = serde_json::to_value(&bootstrap).unwrap_or(Value::Null);
                    let bootstrap_frame = OutboundFrame::Response(OutboundResponse::new(id.clone(), result));
                    // If the bootstrap response can't reach stdout there is no
                    // point pumping the JetStream loop — the caller never saw
                    // the opening `result` and would interpret subsequent
                    // notifications as unsolicited. Bail before consuming the
                    // stream so events stay on the server.
                    if tx.send(bootstrap_frame).await.is_err() {
                        return;
                    }

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
                    let _ = tx.send(make_with_id(*f, &id)).await;
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
                    let _ = tx.send(make_with_id(*f, &id)).await;
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
                    let _ = tx.send(make_with_id(*f, &id)).await;
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
                    let _ = tx.send(make_with_id(*f, &id)).await;
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
                    // Same rationale as message/stream — without the snapshot
                    // landing on stdout, the caller has no anchor to attach
                    // the resubscribe notifications to.
                    if tx.send(bootstrap_frame).await.is_err() {
                        return;
                    }

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
                    let _ = tx.send(make_with_id(*f, &id)).await;
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
                    let _ = tx.send(make_with_id(*f, &id)).await;
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
                    let _ = tx.send(make_with_id(*f, &id)).await;
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
                    let _ = tx.send(make_with_id(*f, &id)).await;
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
mod tests;
