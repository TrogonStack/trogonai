use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use bytes::Bytes;
use futures::StreamExt as _;
use tokio_util::sync::CancellationToken;
use tracing::{instrument, warn};

use crate::agent::handler::{A2aError, A2aHandler};
use crate::agent::wire::{JsonRpcErrorResponse, JsonRpcResponse, parse_request};
use crate::agent::PrincipalCarrier;
use crate::agent_id::A2aAgentId;
use crate::audit::emitter::AuditEmitter;
use crate::audit::task_lifecycle::TaskLifecycleEnvelope;
use crate::jsonrpc::JsonRpcId;
use crate::nats::subjects::task::TaskEventsSubject;
use crate::push::CallerId;
use crate::push::PushDeliverySemanticsRegistry;
use crate::push::dispatcher::{DispatchError, PushDispatcher, maybe_terminal_push_idempotency_key};
use crate::push::push_notification_config_id::PushNotificationConfigId;
use crate::push::push_payload::augment_terminal_push_notification_bytes;
use crate::push::terminal_push_task_state::TerminalPushTaskState;
use crate::req_id::ReqId;
use crate::task_id::A2aTaskId;

/// Handles `message/stream`.
///
/// Flow:
/// 1. Parse and call the handler, which returns `(initial_task, event_stream)`.
/// 2. Reply to the client immediately with the initial task snapshot (bootstrap reply).
/// 3. Spawn a background task that drains the event stream and publishes each event to
///    `TaskEventsSubject` via JetStream for client consumption.
///
/// The background task respects a `CancellationToken` so that `tasks/cancel` can stop
/// the stream pump if the task is terminated externally. The caller (Bridge) is responsible
/// for registering and cleaning up the cancellation token.
#[allow(clippy::too_many_arguments)]
#[instrument(
    name = "a2a.agent.message_stream",
    skip(
        handler,
        payload,
        reply_subject,
        nats,
        js,
        dispatcher,
        cancel,
        audit_emitter,
        push_delivery_semantics,
        principal_carrier
    )
)]
pub async fn handle<H, N, J, D>(
    handler: Arc<H>,
    payload: &[u8],
    reply_subject: Option<String>,
    nats: &N,
    js: &J,
    prefix: &crate::a2a_prefix::A2aPrefix,
    audit_emitter: Arc<dyn AuditEmitter>,
    agent_id: Arc<A2aAgentId>,
    dispatcher: Arc<D>,
    push_delivery_semantics: Arc<PushDeliverySemanticsRegistry>,
    cancel: CancellationToken,
    principal_carrier: PrincipalCarrier,
) -> Option<(A2aTaskId, tokio::task::JoinHandle<()>)>
where
    H: A2aHandler,
    N: trogon_nats::PublishClient + Clone + Send + 'static,
    J: trogon_nats::jetstream::JetStreamPublisher + Clone + Send + 'static,
    D: PushDispatcher + ?Sized,
{
    let Some(reply) = reply_subject else {
        warn!("message/stream received without reply subject; dropping");
        return None;
    };

    let req = match parse_request::<a2a_types::SendMessageRequest>(payload) {
        Ok(r) => r,
        Err(_) => {
            send_reply_error(nats, &reply, None, A2aError::internal("parse error")).await;
            return None;
        }
    };
    let id = req.id.clone();
    let params = match req.params {
        Some(p) => p,
        None => {
            send_reply_error(nats, &reply, id, A2aError::internal("missing params")).await;
            return None;
        }
    };

    let (initial_task, event_stream) = match handler.message_stream(params).await {
        Ok(pair) => pair,
        Err(e) => {
            send_reply_error(nats, &reply, id, e).await;
            return None;
        }
    };

    let task_id = match A2aTaskId::new(initial_task.id.clone()) {
        Ok(t) => t,
        Err(e) => {
            send_reply_error(
                nats,
                &reply,
                id,
                A2aError::invalid_agent_response(format!("bad task id: {e}")),
            )
            .await;
            return None;
        }
    };

    let bootstrap_task_state = initial_task.status.as_ref().map(|s| s.state).unwrap_or(0);

    let json_rpc_req_id = id.as_ref().map(|jid| jid.to_string());

    let bootstrap_bytes = match JsonRpcResponse::new(id, &initial_task).to_bytes() {
        Ok(b) => b,
        Err(e) => {
            warn!(error = %e, "failed to serialize message/stream bootstrap reply");
            return None;
        }
    };

    let headers = async_nats::HeaderMap::new();
    if let Err(e) = nats
        .publish_with_headers(async_nats::Subject::from(reply.as_str()), headers, bootstrap_bytes)
        .await
    {
        warn!(error = %e, "failed to publish message/stream bootstrap reply");
        return None;
    }

    let req_id = ReqId::new();
    let events_subject = TaskEventsSubject::new(prefix, &task_id, &req_id);
    let js_clone = js.clone();
    let task_id_clone = task_id.clone();
    let audit_emitter_clone = audit_emitter.clone();
    let prefix_clone = prefix.clone();
    let agent_id_clone = agent_id.clone();
    let dlq_caller = principal_carrier.push_dlq_caller_id();

    let push_delivery_clone = Arc::clone(&push_delivery_semantics);

    let pump_handle = tokio::spawn(async move {
        pump_events(
            event_stream,
            js_clone,
            events_subject,
            task_id_clone,
            handler,
            dispatcher,
            push_delivery_clone,
            cancel,
            audit_emitter_clone,
            prefix_clone,
            agent_id_clone,
            json_rpc_req_id,
            bootstrap_task_state,
            dlq_caller,
        )
        .await;
    });

    Some((task_id, pump_handle))
}

#[allow(clippy::too_many_arguments)] // internal JetStream/event pump closure — bundled refactor deferred
async fn pump_events<J, H, D>(
    mut stream: crate::agent::handler::TaskEventStream,
    js: J,
    subject: TaskEventsSubject,
    task_id: A2aTaskId,
    handler: Arc<H>,
    dispatcher: Arc<D>,
    push_delivery_semantics: Arc<PushDeliverySemanticsRegistry>,
    cancel: CancellationToken,
    audit_emitter: Arc<dyn AuditEmitter>,
    prefix: crate::a2a_prefix::A2aPrefix,
    agent_id: Arc<A2aAgentId>,
    json_rpc_req_id: Option<String>,
    bootstrap_task_state: i32,
    push_dlq_caller_id: CallerId,
) where
    J: trogon_nats::jetstream::JetStreamPublisher + Clone + Send + Sync,
    H: A2aHandler,
    D: PushDispatcher + ?Sized,
{
    let mut prev_task_state = bootstrap_task_state;
    loop {
        tokio::select! {
            _ = cancel.cancelled() => {
                break;
            }
            event = stream.next() => {
                match event {
                    None => break,
                    Some(Err(e)) => {
                        warn!(task_id = %task_id, error = %e, "event stream error; stopping pump");
                        break;
                    }
                    Some(Ok(ev)) => {
                        if let Some(new_state) = status_update_task_state(&ev)
                            && new_state != prev_task_state
                        {
                            let emitted_at = SystemTime::now()
                                .duration_since(UNIX_EPOCH)
                                .unwrap_or_default()
                                .as_millis()
                                as u64;
                            let env = TaskLifecycleEnvelope::new(
                                agent_id.as_ref(),
                                task_id.as_str(),
                                json_rpc_req_id.clone(),
                                prev_task_state,
                                new_state,
                                emitted_at,
                            );
                            audit_emitter.publish_task_lifecycle(&prefix, agent_id.as_ref(), env).await;
                            prev_task_state = new_state;
                        }

                        let bytes = match serde_json::to_vec(&ev) {
                            Ok(b) => Bytes::from(b),
                            Err(e) => {
                                warn!(task_id = %task_id, error = %e, "failed to serialize event; skipping");
                                continue;
                            }
                        };

                        if is_terminal_status_update(&ev) {
                            dispatch_push_notifications(
                                &task_id,
                                handler.as_ref(),
                                dispatcher.as_ref(),
                                &bytes,
                                &ev,
                                Arc::clone(&push_delivery_semantics),
                                &js,
                                &prefix,
                                &push_dlq_caller_id,
                            )
                            .await;
                        }

                        let headers = async_nats::HeaderMap::new();
                        match js
                            .publish_with_headers(
                                async_nats::Subject::from(subject.to_string().as_str()),
                                headers,
                                bytes,
                            )
                            .await
                        {
                            Ok(ack_future) => {
                                if let Err(e) = ack_future.await {
                                    warn!(task_id = %task_id, error = %e, "JetStream ack failed for event");
                                }
                            }
                            Err(e) => {
                                warn!(task_id = %task_id, error = %e, "failed to publish event to JetStream");
                            }
                        }
                    }
                }
            }
        }
    }
}

fn status_update_task_state(ev: &a2a_types::StreamResponse) -> Option<i32> {
    use a2a_types::stream_response::Payload;
    match &ev.payload {
        Some(Payload::StatusUpdate(update)) => update.status.as_ref().map(|s| s.state),
        _ => None,
    }
}

fn is_terminal_status_update(ev: &a2a_types::StreamResponse) -> bool {
    use a2a_types::TaskState;
    use a2a_types::stream_response::Payload;
    match &ev.payload {
        Some(Payload::StatusUpdate(update)) => {
            let state = update.status.as_ref().map(|s| s.state).unwrap_or(0);
            matches!(
                TaskState::try_from(state),
                Ok(TaskState::Completed) | Ok(TaskState::Failed) | Ok(TaskState::Canceled) | Ok(TaskState::Rejected)
            )
        }
        _ => false,
    }
}

#[allow(clippy::too_many_arguments)]
async fn dispatch_push_notifications<H, D, J>(
    task_id: &A2aTaskId,
    handler: &H,
    dispatcher: &D,
    delivery_payload_bytes: &[u8],
    ev: &a2a_types::StreamResponse,
    push_delivery_semantics: Arc<PushDeliverySemanticsRegistry>,
    js: &J,
    prefix: &crate::a2a_prefix::A2aPrefix,
    push_dlq_caller_id: &CallerId,
) where
    H: A2aHandler,
    D: PushDispatcher + ?Sized,
    J: trogon_nats::jetstream::JetStreamPublisher + Clone + Send + Sync,
{
    let Some(terminal_state) = TerminalPushTaskState::from_stream_terminal_response(ev) else {
        warn!(
            task_id = %task_id,
            "skipped terminal push dispatch: stream response lacked a terminal TaskState classification"
        );
        return;
    };

    let list_req = a2a_types::ListTaskPushNotificationConfigsRequest {
        task_id: task_id.as_str().to_owned(),
        ..Default::default()
    };
    let configs = match handler.push_notification_list(list_req).await {
        Ok(resp) => resp.configs,
        Err(e) => {
            warn!(task_id = %task_id, error = %e, "failed to list push notification configs; skipping dispatch");
            return;
        }
    };
    for config in &configs {
        let cfg_id_res = PushNotificationConfigId::new(config.id.clone());
        let semantics = cfg_id_res
            .as_ref()
            .map(|cid| push_delivery_semantics.get(task_id, cid))
            .unwrap_or_default();

        let derived_key_result = maybe_terminal_push_idempotency_key(config, task_id, &semantics, terminal_state);

        let derived_opt = match &derived_key_result {
            Ok(ok) => ok,
            Err(prep_err) => {
                let dispatch_err = DispatchError::Prep(prep_err.clone());
                warn!(
                    task_id = %task_id,
                    config_id = %config.id,
                    error = %dispatch_err,
                    "push notification dispatch prep failed"
                );
                crate::push::dlq::publish_push_delivery_failure(
                    js,
                    prefix,
                    push_dlq_caller_id,
                    task_id,
                    config,
                    delivery_payload_bytes,
                    &dispatch_err,
                    None,
                )
                .await;
                continue;
            }
        };

        let augmented =
            match augment_terminal_push_notification_bytes(delivery_payload_bytes, &semantics, derived_opt.as_ref()) {
                Ok(c) => c,
                Err(e) => {
                    warn!(
                        task_id = %task_id,
                        config_id = %config.id,
                        error = %e,
                        "failed to augment push notification JSON; skipping"
                    );
                    continue;
                }
            };

        match dispatcher
            .dispatch(task_id, config, semantics, terminal_state, augmented.as_ref())
            .await
        {
            Ok(()) => {}
            Err(ref e) => {
                warn!(task_id = %task_id, config_id = %config.id, error = %e, "push notification delivery failed");
                crate::push::dlq::publish_push_delivery_failure(
                    js,
                    prefix,
                    push_dlq_caller_id,
                    task_id,
                    config,
                    augmented.as_ref(),
                    e,
                    derived_opt.as_ref(),
                )
                .await;
            }
        }
    }
}

async fn send_reply_error<N: trogon_nats::PublishClient>(nats: &N, reply: &str, id: Option<JsonRpcId>, err: A2aError) {
    let bytes = match JsonRpcErrorResponse::new(id, err.code, err.message).to_bytes() {
        Ok(b) => b,
        Err(e) => {
            warn!(error = %e, "failed to serialize message/stream error reply");
            return;
        }
    };
    let headers = async_nats::HeaderMap::new();
    if let Err(e) = nats
        .publish_with_headers(async_nats::Subject::from(reply), headers, bytes)
        .await
    {
        warn!(error = %e, "failed to publish message/stream error reply");
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::a2a_prefix::A2aPrefix;
    use crate::agent::handler::{A2aError, A2aHandler, TaskEventStream};
    use crate::agent::test_support::{make_task, parse_response, rpc_payload};
    use crate::agent_id::A2aAgentId;
    use crate::audit::emitter::{AuditEmitter, NoopAuditEmitter};
    use crate::push::dispatcher::tests::MockPushDispatcher;
    use futures::stream;
    use trogon_nats::AdvancedMockNatsClient;
    use trogon_nats::jetstream::MockJetStreamPublisher;

    fn mock_dispatcher() -> Arc<MockPushDispatcher> {
        Arc::new(MockPushDispatcher::new())
    }

    fn stream_test_agent() -> Arc<A2aAgentId> {
        Arc::new(A2aAgentId::new("stream-agent").unwrap())
    }

    fn noop_audit_emitter() -> Arc<dyn AuditEmitter> {
        Arc::new(NoopAuditEmitter)
    }

    struct StreamingHandler {
        task: a2a_types::Task,
        events: Vec<a2a_types::StreamResponse>,
        push_configs: Vec<a2a_types::TaskPushNotificationConfig>,
    }

    #[async_trait::async_trait]
    impl A2aHandler for StreamingHandler {
        async fn message_send(
            &self,
            _req: a2a_types::SendMessageRequest,
        ) -> Result<a2a_types::SendMessageResponse, A2aError> {
            Err(A2aError::unsupported_operation("stub"))
        }

        async fn message_stream(
            &self,
            _req: a2a_types::SendMessageRequest,
        ) -> Result<(a2a_types::Task, TaskEventStream), A2aError> {
            let events: Vec<Result<a2a_types::StreamResponse, A2aError>> =
                self.events.iter().cloned().map(Ok).collect();
            Ok((self.task.clone(), Box::pin(stream::iter(events))))
        }

        async fn tasks_get(&self, _req: a2a_types::GetTaskRequest) -> Result<a2a_types::Task, A2aError> {
            Err(A2aError::unsupported_operation("stub"))
        }
        async fn tasks_list(
            &self,
            _req: a2a_types::ListTasksRequest,
        ) -> Result<a2a_types::ListTasksResponse, A2aError> {
            Err(A2aError::unsupported_operation("stub"))
        }
        async fn tasks_cancel(&self, _req: a2a_types::CancelTaskRequest) -> Result<a2a_types::Task, A2aError> {
            Err(A2aError::unsupported_operation("stub"))
        }
        async fn tasks_resubscribe(
            &self,
            _req: a2a_types::SubscribeToTaskRequest,
        ) -> Result<a2a_types::Task, A2aError> {
            Err(A2aError::unsupported_operation("stub"))
        }
        async fn push_notification_set(
            &self,
            _req: a2a_types::TaskPushNotificationConfig,
        ) -> Result<a2a_types::TaskPushNotificationConfig, A2aError> {
            Err(A2aError::unsupported_operation("stub"))
        }
        async fn push_notification_get(
            &self,
            _req: a2a_types::GetTaskPushNotificationConfigRequest,
        ) -> Result<a2a_types::TaskPushNotificationConfig, A2aError> {
            Err(A2aError::unsupported_operation("stub"))
        }
        async fn push_notification_list(
            &self,
            _req: a2a_types::ListTaskPushNotificationConfigsRequest,
        ) -> Result<a2a_types::ListTaskPushNotificationConfigsResponse, A2aError> {
            Ok(a2a_types::ListTaskPushNotificationConfigsResponse {
                configs: self.push_configs.clone(),
                ..Default::default()
            })
        }
        async fn push_notification_delete(
            &self,
            _req: a2a_types::DeleteTaskPushNotificationConfigRequest,
        ) -> Result<(), A2aError> {
            Err(A2aError::unsupported_operation("stub"))
        }
        async fn agent_card(
            &self,
            _req: a2a_types::GetExtendedAgentCardRequest,
        ) -> Result<a2a_types::AgentCard, A2aError> {
            Err(A2aError::unsupported_operation("stub"))
        }
    }

    struct FailingStreamHandler;

    #[async_trait::async_trait]
    impl A2aHandler for FailingStreamHandler {
        async fn message_stream(
            &self,
            _req: a2a_types::SendMessageRequest,
        ) -> Result<(a2a_types::Task, TaskEventStream), A2aError> {
            Err(A2aError::internal("handler failed"))
        }
        async fn message_send(
            &self,
            _req: a2a_types::SendMessageRequest,
        ) -> Result<a2a_types::SendMessageResponse, A2aError> {
            Err(A2aError::unsupported_operation("stub"))
        }
        async fn tasks_get(&self, _req: a2a_types::GetTaskRequest) -> Result<a2a_types::Task, A2aError> {
            Err(A2aError::unsupported_operation("stub"))
        }
        async fn tasks_list(
            &self,
            _req: a2a_types::ListTasksRequest,
        ) -> Result<a2a_types::ListTasksResponse, A2aError> {
            Err(A2aError::unsupported_operation("stub"))
        }
        async fn tasks_cancel(&self, _req: a2a_types::CancelTaskRequest) -> Result<a2a_types::Task, A2aError> {
            Err(A2aError::unsupported_operation("stub"))
        }
        async fn tasks_resubscribe(
            &self,
            _req: a2a_types::SubscribeToTaskRequest,
        ) -> Result<a2a_types::Task, A2aError> {
            Err(A2aError::unsupported_operation("stub"))
        }
        async fn push_notification_set(
            &self,
            _req: a2a_types::TaskPushNotificationConfig,
        ) -> Result<a2a_types::TaskPushNotificationConfig, A2aError> {
            Err(A2aError::unsupported_operation("stub"))
        }
        async fn push_notification_get(
            &self,
            _req: a2a_types::GetTaskPushNotificationConfigRequest,
        ) -> Result<a2a_types::TaskPushNotificationConfig, A2aError> {
            Err(A2aError::unsupported_operation("stub"))
        }
        async fn push_notification_list(
            &self,
            _req: a2a_types::ListTaskPushNotificationConfigsRequest,
        ) -> Result<a2a_types::ListTaskPushNotificationConfigsResponse, A2aError> {
            Err(A2aError::unsupported_operation("stub"))
        }
        async fn push_notification_delete(
            &self,
            _req: a2a_types::DeleteTaskPushNotificationConfigRequest,
        ) -> Result<(), A2aError> {
            Err(A2aError::unsupported_operation("stub"))
        }
        async fn agent_card(
            &self,
            _req: a2a_types::GetExtendedAgentCardRequest,
        ) -> Result<a2a_types::AgentCard, A2aError> {
            Err(A2aError::unsupported_operation("stub"))
        }
    }

    fn prefix() -> A2aPrefix {
        A2aPrefix::new("a2a".to_string()).unwrap()
    }

    fn mock_js() -> MockJetStreamPublisher {
        MockJetStreamPublisher::new()
    }

    #[tokio::test]
    async fn bootstrap_reply_contains_task_id() {
        let nats = AdvancedMockNatsClient::new();
        let js = mock_js();
        let handler = Arc::new(StreamingHandler {
            task: make_task("stream-t1"),
            events: vec![],
            push_configs: vec![],
        });
        let result = handle(
            handler,
            &rpc_payload("message/stream", 1),
            Some("reply".into()),
            &nats,
            &js,
            &prefix(),
            noop_audit_emitter(),
            stream_test_agent(),
            mock_dispatcher(),
            Arc::new(crate::push::PushDeliverySemanticsRegistry::default()),
            CancellationToken::new(),
            PrincipalCarrier::absent(CallerId::default()),
        )
        .await;

        assert!(result.is_some());
        let (task_id, pump) = result.unwrap();
        assert_eq!(task_id.as_str(), "stream-t1");
        pump.await.unwrap();

        let body = parse_response(&nats.published_payloads()[0]);
        assert_eq!(body["result"]["id"], "stream-t1");
    }

    #[tokio::test]
    async fn handler_error_sends_error_reply() {
        let nats = AdvancedMockNatsClient::new();
        let js = mock_js();
        let result = handle(
            Arc::new(FailingStreamHandler),
            &rpc_payload("message/stream", 2),
            Some("reply".into()),
            &nats,
            &js,
            &prefix(),
            noop_audit_emitter(),
            stream_test_agent(),
            mock_dispatcher(),
            Arc::new(crate::push::PushDeliverySemanticsRegistry::default()),
            CancellationToken::new(),
            PrincipalCarrier::absent(CallerId::default()),
        )
        .await;
        assert!(result.is_none());
        let body = parse_response(&nats.published_payloads()[0]);
        assert!(body.get("error").is_some());
    }

    #[tokio::test]
    async fn no_reply_subject_returns_none() {
        let nats = AdvancedMockNatsClient::new();
        let js = mock_js();
        let handler = Arc::new(StreamingHandler {
            task: make_task("t"),
            events: vec![],
            push_configs: vec![],
        });
        let result = handle(
            handler,
            &rpc_payload("message/stream", 3),
            None,
            &nats,
            &js,
            &prefix(),
            noop_audit_emitter(),
            stream_test_agent(),
            mock_dispatcher(),
            Arc::new(crate::push::PushDeliverySemanticsRegistry::default()),
            CancellationToken::new(),
            PrincipalCarrier::absent(CallerId::default()),
        )
        .await;
        assert!(result.is_none());
        assert!(nats.published_messages().is_empty());
    }

    #[tokio::test]
    async fn cancellation_stops_pump() {
        let nats = AdvancedMockNatsClient::new();
        let js = mock_js();
        let cancel = CancellationToken::new();

        let handler = Arc::new(StreamingHandler {
            task: make_task("t-cancel"),
            events: vec![],
            push_configs: vec![],
        });
        let result = handle(
            handler,
            &rpc_payload("message/stream", 4),
            Some("reply".into()),
            &nats,
            &js,
            &prefix(),
            noop_audit_emitter(),
            stream_test_agent(),
            mock_dispatcher(),
            Arc::new(crate::push::PushDeliverySemanticsRegistry::default()),
            cancel.clone(),
            PrincipalCarrier::absent(CallerId::default()),
        )
        .await;

        let (_task_id, pump) = result.unwrap();
        cancel.cancel();
        pump.await.unwrap();
    }

    #[tokio::test]
    async fn events_published_to_jetstream() {
        let nats = AdvancedMockNatsClient::new();
        let js = mock_js();

        let event = a2a_types::StreamResponse {
            payload: Some(a2a_types::stream_response::Payload::Task(make_task("t-event"))),
        };
        let handler = Arc::new(StreamingHandler {
            task: make_task("t-event"),
            events: vec![event],
            push_configs: vec![],
        });

        let result = handle(
            handler,
            &rpc_payload("message/stream", 5),
            Some("reply".into()),
            &nats,
            &js,
            &prefix(),
            noop_audit_emitter(),
            stream_test_agent(),
            mock_dispatcher(),
            Arc::new(crate::push::PushDeliverySemanticsRegistry::default()),
            CancellationToken::new(),
            PrincipalCarrier::absent(CallerId::default()),
        )
        .await;

        let (_task_id, pump) = result.unwrap();
        pump.await.unwrap();

        let published = js.published_messages();
        assert!(!published.is_empty(), "expected at least one JetStream publish");
        assert!(published[0].subject.contains("a2a.task.t-event.events."));
    }

    #[tokio::test]
    async fn terminal_status_event_triggers_push_dispatch_attempt() {
        use a2a_types::{TaskState, TaskStatus};

        let nats = AdvancedMockNatsClient::new();
        let js = mock_js();

        let terminal_event = a2a_types::StreamResponse {
            payload: Some(a2a_types::stream_response::Payload::StatusUpdate(
                a2a_types::TaskStatusUpdateEvent {
                    task_id: "t-push".to_string(),
                    status: Some(TaskStatus {
                        state: TaskState::Completed as i32,
                        ..Default::default()
                    }),
                    ..Default::default()
                },
            )),
        };

        let dispatcher = Arc::new(MockPushDispatcher::new());
        let handler = Arc::new(StreamingHandler {
            task: make_task("t-push"),
            events: vec![terminal_event],
            push_configs: vec![],
        });

        let result = handle(
            handler,
            &rpc_payload("message/stream", 6),
            Some("reply".into()),
            &nats,
            &js,
            &prefix(),
            noop_audit_emitter(),
            stream_test_agent(),
            Arc::clone(&dispatcher),
            Arc::new(crate::push::PushDeliverySemanticsRegistry::default()),
            CancellationToken::new(),
            PrincipalCarrier::absent(CallerId::default()),
        )
        .await;

        let (_task_id, pump) = result.unwrap();
        pump.await.unwrap();

        // No push configs; dispatch path is exercised but skips before calling the dispatcher.
        let calls = dispatcher.recorded_calls();
        assert_eq!(calls.len(), 0);
    }

    #[tokio::test]
    async fn terminal_push_delivery_failure_writes_push_dlq() {
        use a2a_types::{TaskPushNotificationConfig, TaskState, TaskStatus};

        let nats = AdvancedMockNatsClient::new();
        let js = mock_js();

        let terminal_event = a2a_types::StreamResponse {
            payload: Some(a2a_types::stream_response::Payload::StatusUpdate(
                a2a_types::TaskStatusUpdateEvent {
                    task_id: "dlq-task".to_string(),
                    status: Some(TaskStatus {
                        state: TaskState::Completed as i32,
                        ..Default::default()
                    }),
                    ..Default::default()
                },
            )),
        };

        let push_cfg = TaskPushNotificationConfig {
            id: "pcfg-1".to_string(),
            url: "https://example.com/webhook".to_string(),
            ..Default::default()
        };

        let dispatcher = Arc::new(MockPushDispatcher::fail_with("boom"));
        let handler = Arc::new(StreamingHandler {
            task: make_task("dlq-task"),
            events: vec![terminal_event],
            push_configs: vec![push_cfg],
        });

        let result = handle(
            handler,
            &rpc_payload("message/stream", 61),
            Some("reply".into()),
            &nats,
            &js,
            &prefix(),
            noop_audit_emitter(),
            stream_test_agent(),
            Arc::clone(&dispatcher),
            Arc::new(crate::push::PushDeliverySemanticsRegistry::default()),
            CancellationToken::new(),
            PrincipalCarrier::absent(CallerId::default()),
        )
        .await;

        let (_task_id, pump) = result.unwrap();
        pump.await.unwrap();

        let subjects = js.published_subjects();
        let dlq_idx = subjects
            .iter()
            .position(|s| s == "a2a.push.dlq._.dlq-task")
            .unwrap_or_else(|| panic!("missing DLQ publish; got {subjects:?}"));

        let msg: serde_json::Value =
            serde_json::from_slice(&js.published_payloads()[dlq_idx]).expect("DLQ payload JSON");
        assert_eq!(msg["schema"].as_str().unwrap(), crate::push::dlq::PUSH_DLQ_SCHEMA_V1);
        assert_eq!(msg["task_id"], "dlq-task");
        assert_eq!(msg["push_config_id"], "pcfg-1");
        assert!(
            msg["error"].as_str().unwrap().contains("boom"),
            "unexpected error summary: {}",
            msg["error"]
        );
        assert_eq!(dispatcher.recorded_calls().len(), 1);
    }

    #[tokio::test]
    async fn terminal_push_dlq_subject_uses_derived_principal_caller_id() {
        use a2a_auth_callout::SpiceDbPrincipal;
        use a2a_types::{TaskPushNotificationConfig, TaskState, TaskStatus};

        let nats = AdvancedMockNatsClient::new();
        let js = mock_js();

        let terminal_event = a2a_types::StreamResponse {
            payload: Some(a2a_types::stream_response::Payload::StatusUpdate(
                a2a_types::TaskStatusUpdateEvent {
                    task_id: "dlq-task".to_string(),
                    status: Some(TaskStatus {
                        state: TaskState::Completed as i32,
                        ..Default::default()
                    }),
                    ..Default::default()
                },
            )),
        };

        let push_cfg = TaskPushNotificationConfig {
            id: "pcfg-1".to_string(),
            url: "https://example.com/webhook".to_string(),
            ..Default::default()
        };

        let dispatcher = Arc::new(MockPushDispatcher::fail_with("boom"));
        let handler = Arc::new(StreamingHandler {
            task: make_task("dlq-task"),
            events: vec![terminal_event],
            push_configs: vec![push_cfg],
        });

        let caller = PrincipalCarrier::with_principal(
            SpiceDbPrincipal(serde_json::json!({"spicedb_subject": "p.q"})),
            CallerId::default(),
        );

        let result = handle(
            handler,
            &rpc_payload("message/stream", 62),
            Some("reply".into()),
            &nats,
            &js,
            &prefix(),
            noop_audit_emitter(),
            stream_test_agent(),
            Arc::clone(&dispatcher),
            Arc::new(crate::push::PushDeliverySemanticsRegistry::default()),
            CancellationToken::new(),
            caller,
        )
        .await;

        let (_task_id, pump) = result.unwrap();
        pump.await.unwrap();

        let subjects = js.published_subjects();
        let dlq_idx = subjects
            .iter()
            .position(|s| s == "a2a.push.dlq.p_q.dlq-task")
            .unwrap_or_else(|| panic!("missing DLQ publish; got {subjects:?}"));
        let _: serde_json::Value = serde_json::from_slice(&js.published_payloads()[dlq_idx]).expect("DLQ payload JSON");
        assert_eq!(dispatcher.recorded_calls().len(), 1);
    }

    #[tokio::test]
    async fn terminal_push_dlq_subject_from_nats_principal_header() {
        use a2a_types::{TaskPushNotificationConfig, TaskState, TaskStatus};
        use crate::agent::principal_carrier::principal_header_fixture;

        let nats = AdvancedMockNatsClient::new();
        let js = mock_js();

        let terminal_event = a2a_types::StreamResponse {
            payload: Some(a2a_types::stream_response::Payload::StatusUpdate(
                a2a_types::TaskStatusUpdateEvent {
                    task_id: "dlq-task".to_string(),
                    status: Some(TaskStatus {
                        state: TaskState::Completed as i32,
                        ..Default::default()
                    }),
                    ..Default::default()
                },
            )),
        };

        let push_cfg = TaskPushNotificationConfig {
            id: "pcfg-1".to_string(),
            url: "https://example.com/webhook".to_string(),
            ..Default::default()
        };

        let dispatcher = Arc::new(MockPushDispatcher::fail_with("boom"));
        let handler = Arc::new(StreamingHandler {
            task: make_task("dlq-task"),
            events: vec![terminal_event],
            push_configs: vec![push_cfg],
        });

        let headers = principal_header_fixture("hdr.caller");
        let carrier = PrincipalCarrier::from_nats_headers(Some(&headers), CallerId::default());

        let result = handle(
            handler,
            &rpc_payload("message/stream", 64),
            Some("reply".into()),
            &nats,
            &js,
            &prefix(),
            noop_audit_emitter(),
            stream_test_agent(),
            Arc::clone(&dispatcher),
            Arc::new(crate::push::PushDeliverySemanticsRegistry::default()),
            CancellationToken::new(),
            carrier,
        )
        .await;

        let (_task_id, pump) = result.unwrap();
        pump.await.unwrap();

        let subjects = js.published_subjects();
        assert!(
            subjects.iter().any(|s| s == "a2a.push.dlq.hdr_caller.dlq-task"),
            "expected header-derived DLQ subject; got {subjects:?}"
        );
    }

    #[tokio::test]
    async fn terminal_push_dlq_falls_back_when_principal_lacks_spicedb_subject() {
        use a2a_auth_callout::SpiceDbPrincipal;
        use a2a_types::{TaskPushNotificationConfig, TaskState, TaskStatus};

        let nats = AdvancedMockNatsClient::new();
        let js = mock_js();

        let terminal_event = a2a_types::StreamResponse {
            payload: Some(a2a_types::stream_response::Payload::StatusUpdate(
                a2a_types::TaskStatusUpdateEvent {
                    task_id: "dlq-task".to_string(),
                    status: Some(TaskStatus {
                        state: TaskState::Completed as i32,
                        ..Default::default()
                    }),
                    ..Default::default()
                },
            )),
        };

        let push_cfg = TaskPushNotificationConfig {
            id: "pcfg-1".to_string(),
            url: "https://example.com/webhook".to_string(),
            ..Default::default()
        };

        let dispatcher = Arc::new(MockPushDispatcher::fail_with("boom"));
        let handler = Arc::new(StreamingHandler {
            task: make_task("dlq-task"),
            events: vec![terminal_event],
            push_configs: vec![push_cfg],
        });

        let carrier = PrincipalCarrier::with_principal(SpiceDbPrincipal(serde_json::json!({})), CallerId::default());

        let result = handle(
            handler,
            &rpc_payload("message/stream", 63),
            Some("reply".into()),
            &nats,
            &js,
            &prefix(),
            noop_audit_emitter(),
            stream_test_agent(),
            Arc::clone(&dispatcher),
            Arc::new(crate::push::PushDeliverySemanticsRegistry::default()),
            CancellationToken::new(),
            carrier,
        )
        .await;

        let (_task_id, pump) = result.unwrap();
        pump.await.unwrap();

        let subjects = js.published_subjects();
        assert!(
            subjects.iter().any(|s| s == "a2a.push.dlq._.dlq-task"),
            "expected fallback DLQ subject; got {subjects:?}"
        );
    }

    #[test]
    fn non_terminal_states_do_not_trigger_dispatch() {
        use a2a_types::{TaskState, TaskStatus};

        let working_event = a2a_types::StreamResponse {
            payload: Some(a2a_types::stream_response::Payload::StatusUpdate(
                a2a_types::TaskStatusUpdateEvent {
                    task_id: "t".to_string(),
                    status: Some(TaskStatus {
                        state: TaskState::Working as i32,
                        ..Default::default()
                    }),
                    ..Default::default()
                },
            )),
        };
        assert!(!is_terminal_status_update(&working_event));
    }

    #[test]
    fn terminal_states_detected_correctly() {
        use a2a_types::{TaskState, TaskStatus};

        for state in [
            TaskState::Completed,
            TaskState::Failed,
            TaskState::Canceled,
            TaskState::Rejected,
        ] {
            let ev = a2a_types::StreamResponse {
                payload: Some(a2a_types::stream_response::Payload::StatusUpdate(
                    a2a_types::TaskStatusUpdateEvent {
                        task_id: "t".to_string(),
                        status: Some(TaskStatus {
                            state: state as i32,
                            ..Default::default()
                        }),
                        ..Default::default()
                    },
                )),
            };
            assert!(is_terminal_status_update(&ev), "expected terminal for state {state:?}");
        }
    }
}
