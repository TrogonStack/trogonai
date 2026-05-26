use a2a_types::{
    AgentCard, CancelTaskRequest, DeleteTaskPushNotificationConfigRequest, GetExtendedAgentCardRequest,
    GetTaskPushNotificationConfigRequest, GetTaskRequest, ListTaskPushNotificationConfigsRequest,
    ListTaskPushNotificationConfigsResponse, ListTasksRequest, ListTasksResponse, SendMessageRequest,
    SendMessageResponse, SubscribeToTaskRequest, Task, TaskPushNotificationConfig,
};
use a2a_auth_callout::MintedUserJwt;
use trogon_nats::RequestClient;
use trogon_nats::jetstream::{JetStreamCreateConsumer, JetStreamGetStream, JsAck, JsMessageOf, JsMessageRef};

use super::streaming::StreamingRequest;
use crate::a2a_prefix::A2aPrefix;
use crate::agent_id::A2aAgentId;
use crate::config::Config;
use crate::gateway_ingress::gateway_ingress_subject_from_agent_subject;
use crate::nats::subjects::agent::{
    AgentCardSubject, MessageSendSubject, MessageStreamSubject, PushDeleteSubject, PushGetSubject, PushListSubject,
    PushSetSubject, TasksCancelSubject, TasksGetSubject, TasksListSubject, TasksResubscribeSubject,
};
use crate::req_id::ReqId;
use crate::task_id::A2aTaskId;

use crate::catalog::{AgentCardWatchError, AgentCardWatchStream};

use super::error::ClientError;
use super::event_stream::TypedEventStream;
use super::resubscribe::open_resubscribe_stream;
use super::streaming::send_streaming;
use super::unary::send_unary;

/// Where [`Client`] sends JSON-RPC unary/stream bootstrap requests (`{prefix}.agent…` vs `{prefix}.gateway…`).
#[derive(Clone, Debug)]
enum ClientIngressTarget {
    AgentSubjects,
    GatewayIngress(MintedUserJwt),
}

pub struct Client<N, J> {
    config: Config,
    agent_id: A2aAgentId,
    nats: N,
    js: J,
    ingress: ClientIngressTarget,
}

impl<N, J> Client<N, J> {
    pub fn new(config: Config, agent_id: A2aAgentId, nats: N, js: J) -> Self {
        Self {
            config,
            agent_id,
            nats,
            js,
            ingress: ClientIngressTarget::AgentSubjects,
        }
    }

    /// Routes requests through **`a2a-gateway`**: unary / bootstrap NATS **`request`** subjects become
    /// **`{prefix}.gateway.{agent_id}.{method…}`** (`gateway_ingress_subject_from_agent_subject`), then the
    /// gateway forwards to **`{prefix}.agent.{agent_id}.{method…}`**.
    ///
    /// Tenancy is the caller's **NATS Account** (see [`docs/a2a/explanation/architecture.md`](../../../../../docs/a2a/explanation/architecture.md) §Decisions), not an extra subject token.
    ///
    /// **`message/stream`** and **`tasks/resubscribe`** still attach to JetStream **`{prefix}.task.…`** event
    /// subjects after their gateway-routed bootstrap/snapshot RPC; only the JSON-RPC ingress hop uses
    /// `{prefix}.gateway.…`.
    ///
    /// `caller_jwt` is attached as [`CALLER_JWT_HEADER_NAME`](a2a_auth_callout::CALLER_JWT_HEADER_NAME) on every
    /// gateway ingress publish. Refresh and replace the JWT on the client when [`ClientError::GatewayCallerJwtExpired`]
    /// is returned.
    pub fn routing_via_gateway_ingress(mut self, caller_jwt: MintedUserJwt) -> Self {
        self.ingress = ClientIngressTarget::GatewayIngress(caller_jwt);
        self
    }

    /// Default (direct) routing to `{prefix}.agent.{agent_id}.{{method}}` subjects.
    pub fn routing_to_agent(mut self) -> Self {
        self.ingress = ClientIngressTarget::AgentSubjects;
        self
    }

    fn outbound_rpc_subject(&self, agent_subject: String) -> Result<String, ClientError> {
        match &self.ingress {
            ClientIngressTarget::AgentSubjects => Ok(agent_subject),
            ClientIngressTarget::GatewayIngress(_) => gateway_ingress_subject_from_agent_subject(&agent_subject, self.prefix())
                .ok_or(ClientError::InvalidRpcSubjectOverlay),
        }
    }

    fn prefix(&self) -> &A2aPrefix {
        self.config.a2a_prefix_ref()
    }

    fn gateway_caller_jwt(&self) -> Option<&MintedUserJwt> {
        match &self.ingress {
            ClientIngressTarget::AgentSubjects => None,
            ClientIngressTarget::GatewayIngress(jwt) => Some(jwt),
        }
    }

    pub fn agent_id(&self) -> &A2aAgentId {
        &self.agent_id
    }
}

impl<N, J> Client<N, J>
where
    N: RequestClient,
    J: JetStreamGetStream,
    JsMessageOf<J>: JsMessageRef + JsAck<Error: std::fmt::Display + Send + 'static> + Send + 'static,
    <J as JetStreamGetStream>::Stream: Send + 'static,
    <<J as JetStreamGetStream>::Stream as JetStreamCreateConsumer>::Consumer: Send + 'static,
    <<<J as JetStreamGetStream>::Stream as JetStreamCreateConsumer>::Consumer as trogon_nats::jetstream::JetStreamConsumer>::Messages: Send + 'static,
    <<<J as JetStreamGetStream>::Stream as JetStreamCreateConsumer>::Consumer as trogon_nats::jetstream::JetStreamConsumer>::MessagesError: std::fmt::Display + Send + 'static,
    <<<J as JetStreamGetStream>::Stream as JetStreamCreateConsumer>::Consumer as trogon_nats::jetstream::JetStreamConsumer>::StreamError: std::fmt::Display + Send + 'static,
{
    pub async fn message_send(&self, req: &SendMessageRequest) -> Result<SendMessageResponse, ClientError> {
        let subject = self.outbound_rpc_subject(MessageSendSubject::new(self.prefix(), &self.agent_id).to_string())?;
        let req_id = ReqId::new();
        send_unary(
            &self.nats,
            &subject,
            "message/send",
            req,
            &req_id,
            self.config.operation_timeout(),
            self.gateway_caller_jwt(),
        )
        .await
    }

    pub async fn message_stream(
        &self,
        req: &SendMessageRequest,
    ) -> Result<(SendMessageResponse, TypedEventStream), ClientError> {
        let subject = self.outbound_rpc_subject(MessageStreamSubject::new(self.prefix(), &self.agent_id).to_string())?;
        let req_id = ReqId::new();
        let ctx = StreamingRequest {
            nats: &self.nats,
            js: &self.js,
            subject: subject.as_str(),
            method: "message/stream",
            req_id: &req_id,
            prefix: self.prefix(),
            op_timeout: self.config.operation_timeout(),
            gateway_caller_jwt: self.gateway_caller_jwt(),
        };
        send_streaming(ctx, req).await
    }

    pub async fn tasks_get(&self, req: &GetTaskRequest) -> Result<Task, ClientError> {
        let subject = self.outbound_rpc_subject(TasksGetSubject::new(self.prefix(), &self.agent_id).to_string())?;
        let req_id = ReqId::new();
        send_unary(
            &self.nats,
            &subject,
            "tasks/get",
            req,
            &req_id,
            self.config.operation_timeout(),
            self.gateway_caller_jwt(),
        )
        .await
    }

    pub async fn tasks_list(&self, req: &ListTasksRequest) -> Result<ListTasksResponse, ClientError> {
        let subject = self.outbound_rpc_subject(TasksListSubject::new(self.prefix(), &self.agent_id).to_string())?;
        let req_id = ReqId::new();
        send_unary(
            &self.nats,
            &subject,
            "tasks/list",
            req,
            &req_id,
            self.config.operation_timeout(),
            self.gateway_caller_jwt(),
        )
        .await
    }

    pub async fn tasks_cancel(&self, req: &CancelTaskRequest) -> Result<Task, ClientError> {
        let subject = self.outbound_rpc_subject(TasksCancelSubject::new(self.prefix(), &self.agent_id).to_string())?;
        let req_id = ReqId::new();
        send_unary(
            &self.nats,
            &subject,
            "tasks/cancel",
            req,
            &req_id,
            self.config.operation_timeout(),
            self.gateway_caller_jwt(),
        )
        .await
    }

    pub async fn tasks_resubscribe(
        &self,
        task_id: &A2aTaskId,
        last_seq: u64,
    ) -> Result<(Task, TypedEventStream), ClientError> {
        let subject =
            self.outbound_rpc_subject(TasksResubscribeSubject::new(self.prefix(), &self.agent_id).to_string())?;
        let req_id = ReqId::new();
        let req = SubscribeToTaskRequest { id: task_id.as_str().to_owned(), tenant: String::new() };
        let snapshot: Task =
            send_unary(
                &self.nats,
                &subject,
                "tasks/resubscribe",
                &req,
                &req_id,
                self.config.operation_timeout(),
                self.gateway_caller_jwt(),
            )
            .await?;
        let stream = open_resubscribe_stream(&self.js, self.prefix(), task_id, last_seq).await?;
        Ok((snapshot, stream))
    }

    pub async fn push_set(&self, req: &TaskPushNotificationConfig) -> Result<TaskPushNotificationConfig, ClientError> {
        let subject = self.outbound_rpc_subject(PushSetSubject::new(self.prefix(), &self.agent_id).to_string())?;
        let req_id = ReqId::new();
        send_unary(
            &self.nats,
            &subject,
            "tasks/pushNotificationConfig/set",
            req,
            &req_id,
            self.config.operation_timeout(),
            self.gateway_caller_jwt(),
        )
        .await
    }

    pub async fn push_get(
        &self,
        req: &GetTaskPushNotificationConfigRequest,
    ) -> Result<TaskPushNotificationConfig, ClientError> {
        let subject = self.outbound_rpc_subject(PushGetSubject::new(self.prefix(), &self.agent_id).to_string())?;
        let req_id = ReqId::new();
        send_unary(
            &self.nats,
            &subject,
            "tasks/pushNotificationConfig/get",
            req,
            &req_id,
            self.config.operation_timeout(),
            self.gateway_caller_jwt(),
        )
        .await
    }

    pub async fn push_list(
        &self,
        req: &ListTaskPushNotificationConfigsRequest,
    ) -> Result<ListTaskPushNotificationConfigsResponse, ClientError> {
        let subject = self.outbound_rpc_subject(PushListSubject::new(self.prefix(), &self.agent_id).to_string())?;
        let req_id = ReqId::new();
        send_unary(
            &self.nats,
            &subject,
            "tasks/pushNotificationConfig/list",
            req,
            &req_id,
            self.config.operation_timeout(),
            self.gateway_caller_jwt(),
        )
        .await
    }

    pub async fn push_delete(&self, req: &DeleteTaskPushNotificationConfigRequest) -> Result<(), ClientError> {
        let subject = self.outbound_rpc_subject(PushDeleteSubject::new(self.prefix(), &self.agent_id).to_string())?;
        let req_id = ReqId::new();
        send_unary::<N, _, ()>(
            &self.nats,
            &subject,
            "tasks/pushNotificationConfig/delete",
            req,
            &req_id,
            self.config.operation_timeout(),
            self.gateway_caller_jwt(),
        )
        .await
    }

    pub async fn agent_card(&self) -> Result<AgentCard, ClientError> {
        let subject = self.outbound_rpc_subject(AgentCardSubject::new(self.prefix(), &self.agent_id).to_string())?;
        let req_id = ReqId::new();
        let req = GetExtendedAgentCardRequest { tenant: String::new() };
        send_unary(
            &self.nats,
            &subject,
            "agent/getAuthenticatedExtendedCard",
            &req,
            &req_id,
            self.config.operation_timeout(),
            self.gateway_caller_jwt(),
        )
        .await
    }

    pub async fn watch_agent_card(
        &self,
        store: &async_nats::jetstream::kv::Store,
        shutdown: tokio_util::sync::CancellationToken,
    ) -> Result<AgentCardWatchStream, AgentCardWatchError> {
        AgentCardWatchStream::subscribe_agent(store, &self.agent_id, shutdown).await
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use a2a_auth_callout::test_support::mint_test_user_jwt;
    use a2a_types::{Task, TaskState, TaskStatus};
    use trogon_nats::AdvancedMockNatsClient;
    use trogon_nats::jetstream::mocks::{MockJetStreamConsumer, MockJetStreamConsumerFactory};

    fn test_config() -> Config {
        Config::for_test("a2a")
    }

    fn test_agent_id() -> A2aAgentId {
        A2aAgentId::new("bot").unwrap()
    }

    fn make_client(
        nats: AdvancedMockNatsClient,
        js: MockJetStreamConsumerFactory,
    ) -> Client<AdvancedMockNatsClient, MockJetStreamConsumerFactory> {
        Client::new(test_config(), test_agent_id(), nats, js)
    }

    fn task_response(task_id: &str) -> bytes::Bytes {
        let task = Task {
            id: task_id.to_string(),
            status: Some(TaskStatus {
                state: TaskState::Completed.into(),
                message: None,
                timestamp: None,
            }),
            ..Default::default()
        };
        let json = serde_json::json!({
            "jsonrpc": "2.0",
            "id": "ignored",
            "result": task
        });
        serde_json::to_vec(&json).unwrap().into()
    }

    fn send_message_response_bytes(task_id: &str) -> bytes::Bytes {
        let task = Task {
            id: task_id.to_string(),
            status: Some(TaskStatus {
                state: TaskState::Working.into(),
                message: None,
                timestamp: None,
            }),
            ..Default::default()
        };
        let response = SendMessageResponse {
            payload: Some(a2a_types::send_message_response::Payload::Task(task)),
        };
        let json = serde_json::json!({
            "jsonrpc": "2.0",
            "id": "ignored",
            "result": response
        });
        serde_json::to_vec(&json).unwrap().into()
    }

    fn error_response(code: i32, msg: &str) -> bytes::Bytes {
        let json = serde_json::json!({
            "jsonrpc": "2.0",
            "id": "ignored",
            "error": { "code": code, "message": msg }
        });
        serde_json::to_vec(&json).unwrap().into()
    }

    #[tokio::test]
    async fn tasks_get_success() {
        let nats = AdvancedMockNatsClient::new();
        nats.set_response("a2a.agent.bot.tasks.get", task_response("task-1"));

        let client = make_client(nats, MockJetStreamConsumerFactory::new());
        let req = GetTaskRequest {
            id: "task-1".into(),
            tenant: String::new(),
            history_length: None,
        };

        let result = client.tasks_get(&req).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap().id, "task-1");
    }

    #[tokio::test]
    async fn tasks_get_not_found_error() {
        let nats = AdvancedMockNatsClient::new();
        nats.set_response("a2a.agent.bot.tasks.get", error_response(-32001, "not found"));

        let client = make_client(nats, MockJetStreamConsumerFactory::new());
        let req = GetTaskRequest {
            id: "bad".into(),
            tenant: String::new(),
            history_length: None,
        };

        assert!(matches!(client.tasks_get(&req).await, Err(ClientError::TaskNotFound)));
    }

    #[tokio::test]
    async fn tasks_cancel_success() {
        let nats = AdvancedMockNatsClient::new();
        nats.set_response("a2a.agent.bot.tasks.cancel", task_response("task-c"));

        let client = make_client(nats, MockJetStreamConsumerFactory::new());
        let req = CancelTaskRequest {
            id: "task-c".into(),
            tenant: String::new(),
            metadata: None,
        };

        let result = client.tasks_cancel(&req).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn tasks_cancel_not_cancelable_error() {
        let nats = AdvancedMockNatsClient::new();
        nats.set_response("a2a.agent.bot.tasks.cancel", error_response(-32002, "not cancelable"));

        let client = make_client(nats, MockJetStreamConsumerFactory::new());
        let req = CancelTaskRequest {
            id: "task-c".into(),
            tenant: String::new(),
            metadata: None,
        };

        assert!(matches!(
            client.tasks_cancel(&req).await,
            Err(ClientError::TaskNotCancelable)
        ));
    }

    #[tokio::test]
    async fn message_send_success() {
        let nats = AdvancedMockNatsClient::new();
        nats.set_response("a2a.agent.bot.message.send", send_message_response_bytes("task-s"));

        let client = make_client(nats, MockJetStreamConsumerFactory::new());
        let req = SendMessageRequest {
            message: Some(a2a_types::Message {
                message_id: "msg-1".into(),
                role: a2a_types::Role::User.into(),
                parts: vec![],
                ..Default::default()
            }),
            ..Default::default()
        };

        let result = client.message_send(&req).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn message_stream_success() {
        let nats = AdvancedMockNatsClient::new();
        nats.set_response("a2a.agent.bot.message.stream", send_message_response_bytes("task-ss"));

        let js = MockJetStreamConsumerFactory::new();
        let (consumer, _tx) = MockJetStreamConsumer::new();
        js.add_consumer(consumer);

        let client = make_client(nats, js);
        let req = SendMessageRequest {
            message: Some(a2a_types::Message {
                message_id: "msg-2".into(),
                role: a2a_types::Role::User.into(),
                parts: vec![],
                ..Default::default()
            }),
            ..Default::default()
        };

        let result = client.message_stream(&req).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn tasks_resubscribe_returns_snapshot_and_stream() {
        let nats = AdvancedMockNatsClient::new();
        nats.set_response("a2a.agent.bot.tasks.resubscribe", task_response("task-r"));

        let js = MockJetStreamConsumerFactory::new();
        let (consumer, _tx) = MockJetStreamConsumer::new();
        js.add_consumer(consumer);

        let client = make_client(nats, js);
        let task_id = A2aTaskId::new("task-r").unwrap();

        let result = client.tasks_resubscribe(&task_id, 42).await;
        assert!(result.is_ok());
        let (snapshot, stream) = result.unwrap();
        assert_eq!(snapshot.id, "task-r");
        assert_eq!(stream.last_seq(), 42);
    }

    #[tokio::test]
    async fn tasks_resubscribe_rpc_failure_does_not_open_consumer() {
        let nats = AdvancedMockNatsClient::new();
        nats.fail_next_request();

        let js = MockJetStreamConsumerFactory::new();

        let client = make_client(nats, js);
        let task_id = A2aTaskId::new("task-r").unwrap();

        let result = client.tasks_resubscribe(&task_id, 0).await;
        assert!(matches!(result, Err(ClientError::Transport(_))));
    }

    #[tokio::test]
    async fn agent_card_success() {
        let nats = AdvancedMockNatsClient::new();
        let card = AgentCard {
            name: "TestBot".into(),
            description: "A test bot".into(),
            version: "1.0.0".into(),
            capabilities: Some(a2a_types::AgentCapabilities::default()),
            ..Default::default()
        };
        let json = serde_json::json!({
            "jsonrpc": "2.0",
            "id": "ignored",
            "result": card
        });
        nats.set_response("a2a.agent.bot.agent.card", serde_json::to_vec(&json).unwrap().into());

        let client = make_client(nats, MockJetStreamConsumerFactory::new());
        let result = client.agent_card().await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap().name, "TestBot");
    }

    #[tokio::test]
    async fn push_set_unsupported_error() {
        let nats = AdvancedMockNatsClient::new();
        nats.set_response(
            "a2a.agent.bot.tasks.push_notification_config.set",
            error_response(-32003, "not supported"),
        );

        let client = make_client(nats, MockJetStreamConsumerFactory::new());
        let req = TaskPushNotificationConfig {
            url: "https://example.com/hook".into(),
            task_id: "t1".into(),
            ..Default::default()
        };

        assert!(matches!(
            client.push_set(&req).await,
            Err(ClientError::PushNotificationNotSupported)
        ));
    }

    #[tokio::test]
    async fn tasks_list_success() {
        let nats = AdvancedMockNatsClient::new();
        let list_response = a2a_types::ListTasksResponse {
            tasks: vec![],
            next_page_token: String::new(),
            page_size: 0,
            total_size: 0,
        };
        let json = serde_json::json!({
            "jsonrpc": "2.0",
            "id": "ignored",
            "result": list_response
        });
        nats.set_response("a2a.agent.bot.tasks.list", serde_json::to_vec(&json).unwrap().into());

        let client = make_client(nats, MockJetStreamConsumerFactory::new());
        let req = ListTasksRequest::default();

        let result = client.tasks_list(&req).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn transport_error_propagates_from_tasks_get() {
        let nats = AdvancedMockNatsClient::new();
        nats.fail_next_request();

        let client = make_client(nats, MockJetStreamConsumerFactory::new());
        let req = GetTaskRequest {
            id: "t".into(),
            tenant: String::new(),
            history_length: None,
        };

        assert!(matches!(client.tasks_get(&req).await, Err(ClientError::Transport(_))));
    }

    #[tokio::test]
    async fn tasks_get_via_gateway_uses_account_scoped_gateway_subject_not_legacy_tenant_segment() {
        let nats = AdvancedMockNatsClient::new();
        // If the client mistakenly prefixed `acme.`, this mock would answer — it should remain idle.
        nats.set_response("a2a.gateway.acme.bot.tasks.get", task_response("wrong-shape"));
        nats.set_response("a2a.gateway.bot.tasks.get", task_response("task-gw"));

        let jwt = mint_test_user_jwt("gw-caller", "a2a", Duration::from_secs(3600));
        let client = make_client(nats, MockJetStreamConsumerFactory::new()).routing_via_gateway_ingress(jwt);
        let req = GetTaskRequest {
            id: "task-gw".into(),
            tenant: String::new(),
            history_length: None,
        };

        let result = client.tasks_get(&req).await.unwrap();
        assert_eq!(result.id, "task-gw");
    }
}
