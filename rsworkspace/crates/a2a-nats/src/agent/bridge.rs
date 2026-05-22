use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use futures::StreamExt as _;
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use crate::agent::dispatch::A2aMethod;
use crate::agent::handler::A2aHandler;
use crate::agent::{agent_card, message_send, message_stream, push_notification, tasks_cancel, tasks_get, tasks_list, tasks_resubscribe};
use crate::config::Config;
use crate::nats::subjects::wildcards::AgentAllSubject;
use crate::task_id::A2aTaskId;

/// Errors that can occur while running the `Bridge`.
#[derive(Debug)]
pub enum BridgeError {
    Subscribe(String),
}

impl std::fmt::Display for BridgeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Subscribe(msg) => write!(f, "subscribe failed: {msg}"),
        }
    }
}

impl std::error::Error for BridgeError {}

/// Shared cancellation state keyed by task ID.
///
/// When `tasks/cancel` arrives for a streaming task, its `CancellationToken` is cancelled
/// and removed from the map. The event pump in `message_stream` listens for cancellation.
#[derive(Clone, Default)]
struct InFlightTasks(Arc<Mutex<HashMap<A2aTaskId, CancellationToken>>>);

impl InFlightTasks {
    fn register(&self, task_id: A2aTaskId, token: CancellationToken) {
        self.0.lock().unwrap().insert(task_id, token);
    }

    fn cancel(&self, task_id: &A2aTaskId) {
        if let Some(token) = self.0.lock().unwrap().remove(task_id) {
            token.cancel();
        }
    }

    fn remove(&self, task_id: &A2aTaskId) {
        self.0.lock().unwrap().remove(task_id);
    }
}

/// Agent-side bridge that subscribes to the wildcard subject and dispatches
/// inbound JSON-RPC requests to an [`A2aHandler`] implementation.
pub struct Bridge<H, N, J> {
    config: Config,
    handler: Arc<H>,
    nats: N,
    js: J,
    semaphore: Arc<Semaphore>,
}

impl<H, N, J> Bridge<H, N, J>
where
    H: A2aHandler,
    N: trogon_nats::SubscribeClient + trogon_nats::PublishClient + Clone + Send + Sync + 'static,
    J: trogon_nats::jetstream::JetStreamPublisher + Clone + Send + Sync + 'static,
{
    pub fn new(config: Config, handler: H, nats: N, js: J) -> Self {
        let semaphore = Arc::new(Semaphore::new(config.max_concurrent_client_tasks()));
        Self {
            config,
            handler: Arc::new(handler),
            nats,
            js,
            semaphore,
        }
    }

    pub async fn run_with_agent_id(
        self,
        agent_id: &crate::agent_id::A2aAgentId,
        shutdown: CancellationToken,
    ) -> Result<(), BridgeError> {
        let prefix = self.config.a2a_prefix_ref();
        let subject = AgentAllSubject::new(prefix, agent_id);
        let prefix_str = format!("{}.agent.{}", prefix.as_str(), agent_id.as_str());
        let prefix_len = prefix_str.len();

        let mut sub = self
            .nats
            .subscribe(subject)
            .await
            .map_err(|e| BridgeError::Subscribe(e.to_string()))?;

        info!(prefix = %prefix.as_str(), agent_id = %agent_id, "A2A agent bridge started");

        let in_flight = InFlightTasks::default();
        let prefix_arc = Arc::new(prefix.clone());

        loop {
            tokio::select! {
                _ = shutdown.cancelled() => {
                    info!("A2A agent bridge shutting down");
                    break;
                }
                msg = sub.next() => {
                    match msg {
                        None => {
                            warn!("NATS subscription closed unexpectedly");
                            break;
                        }
                        Some(msg) => {
                            let permit = match Arc::clone(&self.semaphore).acquire_owned().await {
                                Ok(p) => p,
                                Err(_) => {
                                    warn!("A2A backpressure semaphore closed; shutting down bridge");
                                    break;
                                }
                            };

                            let method = A2aMethod::from_subject(msg.subject.as_str(), prefix_len);
                            let handler = Arc::clone(&self.handler);
                            let nats = self.nats.clone();
                            let js = self.js.clone();
                            let in_flight = in_flight.clone();
                            let prefix_inner = Arc::clone(&prefix_arc);
                            let payload = msg.payload.to_vec();
                            let reply = msg.reply.map(|s| s.to_string());

                            tokio::spawn(async move {
                                dispatch(
                                    method,
                                    &payload,
                                    reply,
                                    handler,
                                    nats,
                                    js,
                                    in_flight,
                                    &prefix_inner,
                                )
                                .await;
                                drop(permit);
                            });
                        }
                    }
                }
            }
        }

        Ok(())
    }
}

#[allow(clippy::too_many_arguments)]
async fn dispatch<H, N, J>(
    method: Option<A2aMethod>,
    payload: &[u8],
    reply: Option<String>,
    handler: Arc<H>,
    nats: N,
    js: J,
    in_flight: InFlightTasks,
    prefix: &crate::a2a_prefix::A2aPrefix,
) where
    H: A2aHandler,
    N: trogon_nats::PublishClient + Clone + Send + 'static,
    J: trogon_nats::jetstream::JetStreamPublisher + Clone + Send + 'static,
{
    match method {
        None => {
            warn!("unknown A2A method; dropping message");
        }
        Some(A2aMethod::MessageSend) => {
            message_send::handle(handler.as_ref(), payload, reply, &nats).await;
        }
        Some(A2aMethod::MessageStream) => {
            let cancel = CancellationToken::new();
            if let Some((task_id, pump)) =
                message_stream::handle(handler.as_ref(), payload, reply, &nats, &js, prefix, cancel.clone()).await
            {
                in_flight.register(task_id.clone(), cancel);
                tokio::spawn(async move {
                    let _ = pump.await;
                    in_flight.remove(&task_id);
                });
            }
        }
        Some(A2aMethod::TasksGet) => {
            tasks_get::handle(handler.as_ref(), payload, reply, &nats).await;
        }
        Some(A2aMethod::TasksList) => {
            tasks_list::handle(handler.as_ref(), payload, reply, &nats).await;
        }
        Some(A2aMethod::TasksCancel) => {
            if let Some(task_id) = extract_task_id(payload) {
                in_flight.cancel(&task_id);
            }
            tasks_cancel::handle(handler.as_ref(), payload, reply, &nats).await;
        }
        Some(A2aMethod::TasksResubscribe) => {
            tasks_resubscribe::handle(handler.as_ref(), payload, reply, &nats).await;
        }
        Some(A2aMethod::PushNotificationSet) => {
            push_notification::handle_set(handler.as_ref(), payload, reply, &nats).await;
        }
        Some(A2aMethod::PushNotificationGet) => {
            push_notification::handle_get(handler.as_ref(), payload, reply, &nats).await;
        }
        Some(A2aMethod::PushNotificationList) => {
            push_notification::handle_list(handler.as_ref(), payload, reply, &nats).await;
        }
        Some(A2aMethod::PushNotificationDelete) => {
            push_notification::handle_delete(handler.as_ref(), payload, reply, &nats).await;
        }
        Some(A2aMethod::AgentCard) => {
            agent_card::handle(handler.as_ref(), payload, reply, &nats).await;
        }
    }
}

fn extract_task_id(payload: &[u8]) -> Option<A2aTaskId> {
    let v: serde_json::Value = serde_json::from_slice(payload).ok()?;
    let id_str = v.get("params")?.get("id")?.as_str()?;
    A2aTaskId::new(id_str).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::test_support::{make_task, rpc_payload, stub};
    use crate::config::Config;
    use trogon_nats::AdvancedMockNatsClient;
    use trogon_nats::jetstream::MockJetStreamPublisher;

    fn make_bridge(
        nats: &AdvancedMockNatsClient,
    ) -> Bridge<std::sync::Mutex<crate::agent::test_support::StubHandler>, AdvancedMockNatsClient, MockJetStreamPublisher>
    {
        Bridge::new(
            Config::for_test("a2a"),
            stub().into_inner().unwrap().into(),
            nats.clone(),
            MockJetStreamPublisher::new(),
        )
    }

    #[test]
    fn bridge_error_display() {
        let e = BridgeError::Subscribe("no connect".into());
        assert!(e.to_string().contains("no connect"));
    }

    #[test]
    fn bridge_error_is_std_error() {
        let e: Box<dyn std::error::Error> = Box::new(BridgeError::Subscribe("x".into()));
        assert!(e.to_string().contains("x"));
    }

    #[tokio::test]
    async fn run_with_agent_id_shuts_down_on_signal() {
        let nats = AdvancedMockNatsClient::new();
        let _inject = nats.inject_messages();
        let bridge = make_bridge(&nats);
        let agent_id = crate::agent_id::A2aAgentId::new("bot").unwrap();
        let shutdown = CancellationToken::new();
        let shutdown_clone = shutdown.clone();

        let handle = tokio::spawn(async move {
            bridge.run_with_agent_id(&agent_id, shutdown_clone).await
        });

        shutdown.cancel();
        let result = handle.await.unwrap();
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn run_with_agent_id_subscribe_failure_returns_err() {
        let nats = AdvancedMockNatsClient::new();
        let bridge = make_bridge(&nats);
        let agent_id = crate::agent_id::A2aAgentId::new("bot").unwrap();
        let shutdown = CancellationToken::new();

        let result = bridge.run_with_agent_id(&agent_id, shutdown).await;
        assert!(result.is_err());
    }

    #[test]
    fn in_flight_tasks_cancel_removes_entry() {
        let in_flight = InFlightTasks::default();
        let task_id = A2aTaskId::new("t1").unwrap();
        let token = CancellationToken::new();
        in_flight.register(task_id.clone(), token.clone());
        assert!(!token.is_cancelled());
        in_flight.cancel(&task_id);
        assert!(token.is_cancelled());
        assert!(in_flight.0.lock().unwrap().is_empty());
    }

    #[test]
    fn in_flight_tasks_remove_does_not_cancel() {
        let in_flight = InFlightTasks::default();
        let task_id = A2aTaskId::new("t2").unwrap();
        let token = CancellationToken::new();
        in_flight.register(task_id.clone(), token.clone());
        in_flight.remove(&task_id);
        assert!(!token.is_cancelled());
        assert!(in_flight.0.lock().unwrap().is_empty());
    }

    #[test]
    fn in_flight_cancel_unknown_task_is_noop() {
        let in_flight = InFlightTasks::default();
        let task_id = A2aTaskId::new("unknown").unwrap();
        in_flight.cancel(&task_id);
    }

    #[test]
    fn extract_task_id_parses_id_from_params() {
        let payload = serde_json::to_vec(&serde_json::json!({
            "jsonrpc": "2.0", "id": 1, "method": "tasks/cancel",
            "params": {"id": "t-abc"}
        }))
        .unwrap();
        let tid = extract_task_id(&payload);
        assert_eq!(tid.map(|t| t.as_str().to_string()), Some("t-abc".into()));
    }

    #[test]
    fn extract_task_id_returns_none_for_missing_params() {
        let payload = serde_json::to_vec(&serde_json::json!({"jsonrpc": "2.0"})).unwrap();
        assert!(extract_task_id(&payload).is_none());
    }

    #[tokio::test]
    async fn dispatch_unknown_method_is_noop() {
        let nats = AdvancedMockNatsClient::new();
        let handler = Arc::new(stub());
        let in_flight = InFlightTasks::default();
        let prefix = crate::a2a_prefix::A2aPrefix::new("a2a".to_string()).unwrap();
        dispatch(
            None,
            b"{}",
            Some("reply".into()),
            handler,
            nats.clone(),
            MockJetStreamPublisher::new(),
            in_flight,
            &prefix,
        )
        .await;
        assert!(nats.published_messages().is_empty());
    }

    #[tokio::test]
    async fn dispatch_message_send_routes_to_handler() {
        let nats = AdvancedMockNatsClient::new();
        let handler_stub = stub();
        handler_stub.lock().unwrap().message_send_result =
            Some(Ok(a2a_types::SendMessageResponse { payload: None }));
        let handler = Arc::new(handler_stub);
        let in_flight = InFlightTasks::default();
        let prefix = crate::a2a_prefix::A2aPrefix::new("a2a".to_string()).unwrap();
        dispatch(
            Some(A2aMethod::MessageSend),
            &rpc_payload("message/send", 1),
            Some("reply".into()),
            handler,
            nats.clone(),
            MockJetStreamPublisher::new(),
            in_flight,
            &prefix,
        )
        .await;
        assert_eq!(nats.published_messages(), vec!["reply"]);
    }

    #[tokio::test]
    async fn dispatch_tasks_get_routes_to_handler() {
        let nats = AdvancedMockNatsClient::new();
        let handler_stub = stub();
        handler_stub.lock().unwrap().tasks_get_result = Some(Ok(make_task("tg1")));
        let handler = Arc::new(handler_stub);
        let in_flight = InFlightTasks::default();
        let prefix = crate::a2a_prefix::A2aPrefix::new("a2a".to_string()).unwrap();
        dispatch(
            Some(A2aMethod::TasksGet),
            &rpc_payload("tasks/get", 2),
            Some("reply".into()),
            handler,
            nats.clone(),
            MockJetStreamPublisher::new(),
            in_flight,
            &prefix,
        )
        .await;
        let body: serde_json::Value = serde_json::from_slice(&nats.published_payloads()[0]).unwrap();
        assert_eq!(body["result"]["id"], "tg1");
    }

    #[tokio::test]
    async fn concurrency_limit_blocks_excess_handlers() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        struct BlockingHandler {
            // Number of handlers currently inside message_send (incremented at entry, decremented at exit).
            in_flight_count: Arc<AtomicUsize>,
            // Total invocations that completed (incremented just before returning).
            completed_count: Arc<AtomicUsize>,
            // Semaphore with 0 permits; each handler blocks until a permit is added.
            gate: Arc<Semaphore>,
        }

        #[async_trait::async_trait]
        impl crate::agent::handler::A2aHandler for BlockingHandler {
            async fn message_send(
                &self,
                _req: a2a_types::SendMessageRequest,
            ) -> Result<a2a_types::SendMessageResponse, crate::agent::handler::A2aError> {
                self.in_flight_count.fetch_add(1, Ordering::SeqCst);
                let _permit = self.gate.acquire().await.unwrap();
                self.in_flight_count.fetch_sub(1, Ordering::SeqCst);
                self.completed_count.fetch_add(1, Ordering::SeqCst);
                Ok(a2a_types::SendMessageResponse { payload: None })
            }

            async fn message_stream(
                &self,
                _req: a2a_types::SendMessageRequest,
            ) -> Result<(a2a_types::Task, crate::agent::handler::TaskEventStream), crate::agent::handler::A2aError> {
                Err(crate::agent::handler::A2aError::unsupported_operation("not used"))
            }

            async fn tasks_get(
                &self,
                _req: a2a_types::GetTaskRequest,
            ) -> Result<a2a_types::Task, crate::agent::handler::A2aError> {
                Err(crate::agent::handler::A2aError::unsupported_operation("not used"))
            }

            async fn tasks_list(
                &self,
                _req: a2a_types::ListTasksRequest,
            ) -> Result<a2a_types::ListTasksResponse, crate::agent::handler::A2aError> {
                Err(crate::agent::handler::A2aError::unsupported_operation("not used"))
            }

            async fn tasks_cancel(
                &self,
                _req: a2a_types::CancelTaskRequest,
            ) -> Result<a2a_types::Task, crate::agent::handler::A2aError> {
                Err(crate::agent::handler::A2aError::unsupported_operation("not used"))
            }

            async fn tasks_resubscribe(
                &self,
                _req: a2a_types::SubscribeToTaskRequest,
            ) -> Result<a2a_types::Task, crate::agent::handler::A2aError> {
                Err(crate::agent::handler::A2aError::unsupported_operation("not used"))
            }

            async fn push_notification_set(
                &self,
                _req: a2a_types::TaskPushNotificationConfig,
            ) -> Result<a2a_types::TaskPushNotificationConfig, crate::agent::handler::A2aError> {
                Err(crate::agent::handler::A2aError::unsupported_operation("not used"))
            }

            async fn push_notification_get(
                &self,
                _req: a2a_types::GetTaskPushNotificationConfigRequest,
            ) -> Result<a2a_types::TaskPushNotificationConfig, crate::agent::handler::A2aError> {
                Err(crate::agent::handler::A2aError::unsupported_operation("not used"))
            }

            async fn push_notification_list(
                &self,
                _req: a2a_types::ListTaskPushNotificationConfigsRequest,
            ) -> Result<a2a_types::ListTaskPushNotificationConfigsResponse, crate::agent::handler::A2aError> {
                Err(crate::agent::handler::A2aError::unsupported_operation("not used"))
            }

            async fn push_notification_delete(
                &self,
                _req: a2a_types::DeleteTaskPushNotificationConfigRequest,
            ) -> Result<(), crate::agent::handler::A2aError> {
                Err(crate::agent::handler::A2aError::unsupported_operation("not used"))
            }

            async fn agent_card(
                &self,
                _req: a2a_types::GetExtendedAgentCardRequest,
            ) -> Result<a2a_types::AgentCard, crate::agent::handler::A2aError> {
                Err(crate::agent::handler::A2aError::unsupported_operation("not used"))
            }
        }

        let in_flight_count = Arc::new(AtomicUsize::new(0));
        let completed_count = Arc::new(AtomicUsize::new(0));
        let gate = Arc::new(Semaphore::new(0));

        let handler = BlockingHandler {
            in_flight_count: Arc::clone(&in_flight_count),
            completed_count: Arc::clone(&completed_count),
            gate: Arc::clone(&gate),
        };

        let nats = AdvancedMockNatsClient::new();
        let inject = nats.inject_messages();

        let bridge = Bridge::new(
            Config::for_test("a2a").with_max_concurrent_client_tasks(2),
            handler,
            nats.clone(),
            MockJetStreamPublisher::new(),
        );

        let agent_id = crate::agent_id::A2aAgentId::new("bot").unwrap();
        let shutdown = CancellationToken::new();
        let shutdown_clone = shutdown.clone();

        let bridge_handle = tokio::spawn(async move {
            bridge.run_with_agent_id(&agent_id, shutdown_clone).await
        });

        let msg_subject = "a2a.agent.bot.message.send";
        let payload = bytes::Bytes::from(rpc_payload("message/send", 1));

        for _ in 0..3 {
            inject
                .unbounded_send(async_nats::Message {
                    subject: msg_subject.into(),
                    reply: Some("_INBOX.reply".into()),
                    payload: payload.clone(),
                    headers: None,
                    length: payload.len(),
                    status: None,
                    description: None,
                })
                .unwrap();
        }

        // Wait until exactly 2 handlers are in-flight (the 3rd is blocked by the bridge semaphore).
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                if in_flight_count.load(Ordering::SeqCst) == 2 {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("timed out waiting for 2 handlers to be in-flight");

        assert_eq!(in_flight_count.load(Ordering::SeqCst), 2);

        // Release all 3 gates so every in-flight and queued handler can finish.
        gate.add_permits(3);

        // Wait until all 3 messages have been handled end-to-end.
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                if completed_count.load(Ordering::SeqCst) == 3 {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("timed out waiting for all handlers to finish");

        shutdown.cancel();
        bridge_handle.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn permit_released_after_handler_returns() {
        let nats = AdvancedMockNatsClient::new();
        let inject = nats.inject_messages();

        let handler_stub = stub();
        handler_stub.lock().unwrap().message_send_result =
            Some(Ok(a2a_types::SendMessageResponse { payload: None }));

        let config = Config::for_test("a2a").with_max_concurrent_client_tasks(1);
        let semaphore = Arc::new(Semaphore::new(config.max_concurrent_client_tasks()));
        let semaphore_observe = Arc::clone(&semaphore);

        let bridge = Bridge {
            semaphore,
            config,
            handler: Arc::new(handler_stub),
            nats: nats.clone(),
            js: MockJetStreamPublisher::new(),
        };

        let agent_id = crate::agent_id::A2aAgentId::new("bot").unwrap();
        let shutdown = CancellationToken::new();
        let shutdown_clone = shutdown.clone();

        let bridge_handle = tokio::spawn(async move {
            bridge.run_with_agent_id(&agent_id, shutdown_clone).await
        });

        let payload = bytes::Bytes::from(rpc_payload("message/send", 10));
        inject
            .unbounded_send(async_nats::Message {
                subject: "a2a.agent.bot.message.send".into(),
                reply: None,
                payload: payload.clone(),
                headers: None,
                length: payload.len(),
                status: None,
                description: None,
            })
            .unwrap();

        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                if semaphore_observe.available_permits() == 1 {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("timed out waiting for permit to be released");

        assert_eq!(semaphore_observe.available_permits(), 1);

        shutdown.cancel();
        bridge_handle.await.unwrap().unwrap();
    }
}
