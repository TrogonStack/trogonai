use a2a_types::{
    AgentCard, CancelTaskRequest, DeleteTaskPushNotificationConfigRequest, GetExtendedAgentCardRequest,
    GetTaskPushNotificationConfigRequest, GetTaskRequest, ListTaskPushNotificationConfigsRequest,
    ListTaskPushNotificationConfigsResponse, ListTasksRequest, ListTasksResponse, SendMessageRequest,
    SendMessageResponse, Task, TaskPushNotificationConfig,
};
use trogon_nats::RequestClient;
use trogon_nats::jetstream::{JetStreamCreateConsumer, JetStreamGetStream, JsAck, JsMessageOf, JsMessageRef};

use crate::a2a_prefix::A2aPrefix;
use crate::agent_id::A2aAgentId;
use crate::config::Config;
use crate::nats::subjects::agent::{
    AgentCardSubject, MessageSendSubject, MessageStreamSubject, PushDeleteSubject, PushGetSubject, PushListSubject,
    PushSetSubject, TasksCancelSubject, TasksGetSubject, TasksListSubject,
};
use super::streaming::StreamingRequest;
use crate::req_id::ReqId;
use crate::task_id::A2aTaskId;

use super::error::ClientError;
use super::event_stream::TypedEventStream;
use super::resubscribe::open_resubscribe_stream;
use super::streaming::send_streaming;
use super::unary::send_unary;

pub struct Client<N, J> {
    config: Config,
    agent_id: A2aAgentId,
    nats: N,
    js: J,
}

impl<N, J> Client<N, J> {
    pub fn new(config: Config, agent_id: A2aAgentId, nats: N, js: J) -> Self {
        Self { config, agent_id, nats, js }
    }

    fn prefix(&self) -> &A2aPrefix {
        self.config.a2a_prefix_ref()
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
        let subject = MessageSendSubject::new(self.prefix(), &self.agent_id).to_string();
        let req_id = ReqId::new();
        send_unary(&self.nats, &subject, "message/send", req, &req_id, self.config.operation_timeout()).await
    }

    pub async fn message_stream(
        &self,
        req: &SendMessageRequest,
    ) -> Result<(SendMessageResponse, TypedEventStream), ClientError> {
        let subject = MessageStreamSubject::new(self.prefix(), &self.agent_id).to_string();
        let req_id = ReqId::new();
        let ctx = StreamingRequest {
            nats: &self.nats,
            js: &self.js,
            subject: &subject,
            method: "message/stream",
            req_id: &req_id,
            prefix: self.prefix(),
            op_timeout: self.config.operation_timeout(),
        };
        send_streaming(ctx, req).await
    }

    pub async fn tasks_get(&self, req: &GetTaskRequest) -> Result<Task, ClientError> {
        let subject = TasksGetSubject::new(self.prefix(), &self.agent_id).to_string();
        let req_id = ReqId::new();
        send_unary(&self.nats, &subject, "tasks/get", req, &req_id, self.config.operation_timeout()).await
    }

    pub async fn tasks_list(&self, req: &ListTasksRequest) -> Result<ListTasksResponse, ClientError> {
        let subject = TasksListSubject::new(self.prefix(), &self.agent_id).to_string();
        let req_id = ReqId::new();
        send_unary(&self.nats, &subject, "tasks/list", req, &req_id, self.config.operation_timeout()).await
    }

    pub async fn tasks_cancel(&self, req: &CancelTaskRequest) -> Result<Task, ClientError> {
        let subject = TasksCancelSubject::new(self.prefix(), &self.agent_id).to_string();
        let req_id = ReqId::new();
        send_unary(&self.nats, &subject, "tasks/cancel", req, &req_id, self.config.operation_timeout()).await
    }

    pub async fn tasks_resubscribe(
        &self,
        task_id: &A2aTaskId,
        last_seq: u64,
    ) -> Result<TypedEventStream, ClientError> {
        open_resubscribe_stream(&self.js, self.prefix(), task_id, last_seq).await
    }

    pub async fn push_set(&self, req: &TaskPushNotificationConfig) -> Result<TaskPushNotificationConfig, ClientError> {
        let subject = PushSetSubject::new(self.prefix(), &self.agent_id).to_string();
        let req_id = ReqId::new();
        send_unary(
            &self.nats,
            &subject,
            "tasks/pushNotificationConfig/set",
            req,
            &req_id,
            self.config.operation_timeout(),
        )
        .await
    }

    pub async fn push_get(
        &self,
        req: &GetTaskPushNotificationConfigRequest,
    ) -> Result<TaskPushNotificationConfig, ClientError> {
        let subject = PushGetSubject::new(self.prefix(), &self.agent_id).to_string();
        let req_id = ReqId::new();
        send_unary(
            &self.nats,
            &subject,
            "tasks/pushNotificationConfig/get",
            req,
            &req_id,
            self.config.operation_timeout(),
        )
        .await
    }

    pub async fn push_list(
        &self,
        req: &ListTaskPushNotificationConfigsRequest,
    ) -> Result<ListTaskPushNotificationConfigsResponse, ClientError> {
        let subject = PushListSubject::new(self.prefix(), &self.agent_id).to_string();
        let req_id = ReqId::new();
        send_unary(
            &self.nats,
            &subject,
            "tasks/pushNotificationConfig/list",
            req,
            &req_id,
            self.config.operation_timeout(),
        )
        .await
    }

    pub async fn push_delete(&self, req: &DeleteTaskPushNotificationConfigRequest) -> Result<(), ClientError> {
        let subject = PushDeleteSubject::new(self.prefix(), &self.agent_id).to_string();
        let req_id = ReqId::new();
        send_unary::<N, _, ()>(
            &self.nats,
            &subject,
            "tasks/pushNotificationConfig/delete",
            req,
            &req_id,
            self.config.operation_timeout(),
        )
        .await
    }

    pub async fn agent_card(&self) -> Result<AgentCard, ClientError> {
        let subject = AgentCardSubject::new(self.prefix(), &self.agent_id).to_string();
        let req_id = ReqId::new();
        let req = GetExtendedAgentCardRequest { tenant: String::new() };
        send_unary(&self.nats, &subject, "agent/getAuthenticatedExtendedCard", &req, &req_id, self.config.operation_timeout())
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
        let req = GetTaskRequest { id: "task-1".into(), tenant: String::new(), history_length: None };

        let result = client.tasks_get(&req).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap().id, "task-1");
    }

    #[tokio::test]
    async fn tasks_get_not_found_error() {
        let nats = AdvancedMockNatsClient::new();
        nats.set_response("a2a.agent.bot.tasks.get", error_response(-32001, "not found"));

        let client = make_client(nats, MockJetStreamConsumerFactory::new());
        let req = GetTaskRequest { id: "bad".into(), tenant: String::new(), history_length: None };

        assert!(matches!(client.tasks_get(&req).await, Err(ClientError::TaskNotFound)));
    }

    #[tokio::test]
    async fn tasks_cancel_success() {
        let nats = AdvancedMockNatsClient::new();
        nats.set_response("a2a.agent.bot.tasks.cancel", task_response("task-c"));

        let client = make_client(nats, MockJetStreamConsumerFactory::new());
        let req = CancelTaskRequest { id: "task-c".into(), tenant: String::new(), metadata: None };

        let result = client.tasks_cancel(&req).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn tasks_cancel_not_cancelable_error() {
        let nats = AdvancedMockNatsClient::new();
        nats.set_response("a2a.agent.bot.tasks.cancel", error_response(-32002, "not cancelable"));

        let client = make_client(nats, MockJetStreamConsumerFactory::new());
        let req = CancelTaskRequest { id: "task-c".into(), tenant: String::new(), metadata: None };

        assert!(matches!(client.tasks_cancel(&req).await, Err(ClientError::TaskNotCancelable)));
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
    async fn tasks_resubscribe_returns_stream() {
        let nats = AdvancedMockNatsClient::new();
        let js = MockJetStreamConsumerFactory::new();
        let (consumer, _tx) = MockJetStreamConsumer::new();
        js.add_consumer(consumer);

        let client = make_client(nats, js);
        let task_id = A2aTaskId::new("task-r").unwrap();

        let result = client.tasks_resubscribe(&task_id, 42).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap().last_seq(), 42);
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

        assert!(matches!(client.push_set(&req).await, Err(ClientError::PushNotificationNotSupported)));
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
        let req = GetTaskRequest { id: "t".into(), tenant: String::new(), history_length: None };

        assert!(matches!(client.tasks_get(&req).await, Err(ClientError::Transport(_))));
    }
}
