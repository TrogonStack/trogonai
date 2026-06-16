use std::collections::HashMap;
use std::sync::Mutex;

use a2a_nats::server::{A2aError, A2aHandler, TaskEventStream};
use a2a::event::{StreamResponse, TaskStatusUpdateEvent};
use a2a::types::{
    GetTaskRequest, Message, Part, PartContent, Role, SendMessageResponse, Task, TaskPushNotificationConfig,
    TaskState, TaskStatus,
};
use tracing::error;

/// Smoke-only convention: when a message id starts with this prefix, echo treats
/// the suffix as the task id so smoke tests can pre-register a push config for a
/// task that does not yet exist.
const TASK_ID_FROM_MESSAGE_ID_PREFIX: &str = "echo-task:";

fn task_id_from_message_id(message_id: &str) -> String {
    message_id
        .strip_prefix(TASK_ID_FROM_MESSAGE_ID_PREFIX)
        .map(str::to_owned)
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string())
}

struct EchoHandler {
    tasks: Mutex<HashMap<String, Task>>,
    push_configs: Mutex<HashMap<String, Vec<TaskPushNotificationConfig>>>,
}

impl EchoHandler {
    fn new() -> Self {
        Self {
            tasks: Mutex::new(HashMap::new()),
            push_configs: Mutex::new(HashMap::new()),
        }
    }

    fn store_task(&self, task: Task) {
        if let Ok(mut guard) = self.tasks.lock() {
            guard.insert(task.id.clone(), task);
        }
    }
}

fn text_part(text: impl Into<String>) -> Part {
    Part {
        content: PartContent::Text(text.into()),
        filename: None,
        media_type: None,
        metadata: None,
    }
}

fn agent_message(task_id: &str, context_id: &str, text: impl Into<String>) -> Message {
    Message {
        message_id: uuid::Uuid::new_v4().to_string(),
        task_id: Some(task_id.to_owned()),
        context_id: Some(context_id.to_owned()),
        role: Role::Agent,
        parts: vec![text_part(text)],
        metadata: None,
        extensions: None,
        reference_task_ids: None,
    }
}

fn completed_task(task_id: &str, context_id: &str, input: &Message) -> Task {
    let echo_text = input
        .parts
        .first()
        .and_then(|p| {
            if let a2a::types::PartContent::Text(t) = &p.content {
                Some(t.as_str())
            } else {
                None
            }
        })
        .unwrap_or("");

    let reply = agent_message(task_id, context_id, format!("echo: {echo_text}"));

    Task {
        id: task_id.to_owned(),
        context_id: context_id.to_owned(),
        status: TaskStatus {
            state: TaskState::Completed,
            message: None,
            timestamp: None,
        },
        history: Some(vec![input.clone(), reply]),
        artifacts: None,
        metadata: None,
    }
}

#[async_trait::async_trait]
impl A2aHandler for EchoHandler {
    async fn message_send(&self, req: a2a::types::SendMessageRequest) -> Result<SendMessageResponse, A2aError> {
        let input = req.message;
        let task_id = task_id_from_message_id(&input.message_id);
        let context_id = input.context_id.clone().filter(|s| !s.is_empty()).unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

        let task = completed_task(&task_id, &context_id, &input);
        self.store_task(task.clone());

        Ok(SendMessageResponse::Task(task))
    }

    async fn message_stream(&self, req: a2a::types::SendMessageRequest) -> Result<(Task, TaskEventStream), A2aError> {
        let input = req.message;
        let task_id = task_id_from_message_id(&input.message_id);
        let context_id = input.context_id.clone().filter(|s| !s.is_empty()).unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

        let initial = Task {
            id: task_id.clone(),
            context_id: context_id.clone(),
            status: TaskStatus {
                state: TaskState::Submitted,
                message: None,
                timestamp: None,
            },
            artifacts: None,
            history: None,
            metadata: None,
        };

        let working_event = StreamResponse::StatusUpdate(TaskStatusUpdateEvent {
            task_id: task_id.clone(),
            context_id: context_id.clone(),
            status: TaskStatus {
                state: TaskState::Working,
                message: None,
                timestamp: None,
            },
            metadata: None,
        });

        let completed = completed_task(&task_id, &context_id, &input);
        self.store_task(completed.clone());
        let completed_event = StreamResponse::Task(completed);

        let stream: TaskEventStream = Box::pin(futures::stream::iter([Ok(working_event), Ok(completed_event)]));

        Ok((initial, stream))
    }

    async fn tasks_get(&self, req: GetTaskRequest) -> Result<Task, A2aError> {
        let task_id = req.id;
        let guard = self
            .tasks
            .lock()
            .map_err(|_| A2aError::internal("task store lock poisoned"))?;
        guard
            .get(&task_id)
            .cloned()
            .ok_or_else(|| A2aError::task_not_found(&task_id))
    }

    async fn tasks_list(&self, _req: a2a::types::ListTasksRequest) -> Result<a2a::types::ListTasksResponse, A2aError> {
        Err(A2aError::unsupported_operation("not implemented"))
    }

    async fn tasks_cancel(&self, _req: a2a::types::CancelTaskRequest) -> Result<Task, A2aError> {
        Err(A2aError::unsupported_operation("not implemented"))
    }

    async fn tasks_resubscribe(&self, _req: a2a::types::SubscribeToTaskRequest) -> Result<Task, A2aError> {
        Err(A2aError::unsupported_operation("not implemented"))
    }

    async fn push_notification_set(
        &self,
        req: a2a::types::TaskPushNotificationConfig,
    ) -> Result<a2a::types::TaskPushNotificationConfig, A2aError> {
        let task_id = req.task_id.clone();
        if task_id.is_empty() {
            return Err(A2aError::unsupported_operation("task_id required"));
        }
        let mut guard = self
            .push_configs
            .lock()
            .map_err(|_| A2aError::internal("push config store lock poisoned"))?;
        let bucket = guard.entry(task_id).or_default();
        if let Some(slot) = bucket.iter_mut().find(|c| c.id == req.id) {
            *slot = req.clone();
        } else {
            bucket.push(req.clone());
        }
        Ok(req)
    }

    async fn push_notification_get(
        &self,
        req: a2a::types::GetTaskPushNotificationConfigRequest,
    ) -> Result<a2a::types::TaskPushNotificationConfig, A2aError> {
        let guard = self
            .push_configs
            .lock()
            .map_err(|_| A2aError::internal("push config store lock poisoned"))?;
        guard
            .get(&req.task_id)
            .and_then(|bucket| bucket.iter().find(|c| c.id.as_deref() == Some(req.id.as_str())).cloned())
            .ok_or_else(|| A2aError::push_notification_not_supported("config not found"))
    }

    async fn push_notification_list(
        &self,
        req: a2a::types::ListTaskPushNotificationConfigsRequest,
    ) -> Result<a2a::types::ListTaskPushNotificationConfigsResponse, A2aError> {
        let guard = self
            .push_configs
            .lock()
            .map_err(|_| A2aError::internal("push config store lock poisoned"))?;
        let configs = guard.get(&req.task_id).cloned().unwrap_or_default();
        Ok(a2a::types::ListTaskPushNotificationConfigsResponse {
            configs,
            next_page_token: None,
        })
    }

    async fn push_notification_delete(
        &self,
        req: a2a::types::DeleteTaskPushNotificationConfigRequest,
    ) -> Result<(), A2aError> {
        let mut guard = self
            .push_configs
            .lock()
            .map_err(|_| A2aError::internal("push config store lock poisoned"))?;
        if let Some(bucket) = guard.get_mut(&req.task_id) {
            bucket.retain(|c| c.id.as_deref() != Some(req.id.as_str()));
        }
        Ok(())
    }

    async fn agent_card(&self, _req: a2a::types::GetExtendedAgentCardRequest) -> Result<a2a::agent_card::AgentCard, A2aError> {
        Ok(echo_agent_card())
    }
}

fn echo_agent_card() -> a2a::agent_card::AgentCard {
    use std::collections::HashMap;
    let base_url = std::env::var("ECHO_AGENT_BASE_URL").unwrap_or_else(|_| "https://echo.example.com".into());
    let mut security_schemes: HashMap<String, a2a::agent_card::SecurityScheme> = HashMap::new();
    security_schemes.insert(
        "bearer".to_string(),
        a2a::agent_card::SecurityScheme::HttpAuth(a2a::agent_card::HttpAuthSecurityScheme {
            scheme: "bearer".to_string(),
            bearer_format: Some("JWT".to_string()),
            description: Some("OIDC access token".to_string()),
        }),
    );
    a2a::agent_card::AgentCard {
        name: "echo".to_string(),
        description: "Reference A2A echo agent".to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        supported_interfaces: vec![
            a2a::agent_card::AgentInterface {
                url: format!("{base_url}/"),
                protocol_binding: "JSONRPC".to_string(),
                protocol_version: "0.3.0".to_string(),
                tenant: None,
            },
            a2a::agent_card::AgentInterface {
                url: format!("{base_url}/v1"),
                protocol_binding: "HTTP+JSON".to_string(),
                protocol_version: "0.3.0".to_string(),
                tenant: None,
            },
        ],
        capabilities: a2a::agent_card::AgentCapabilities {
            streaming: Some(true),
            push_notifications: Some(true),
            extensions: None,
            extended_agent_card: None,
        },
        default_input_modes: vec!["text/plain".to_string()],
        default_output_modes: vec!["text/plain".to_string()],
        skills: vec![],
        provider: None,
        documentation_url: None,
        icon_url: None,
        security_schemes: Some(security_schemes),
        security_requirements: Some(vec![{
            let mut m = HashMap::new();
            m.insert("bearer".to_string(), vec![]);
            m
        }]),
        signatures: None,
    }
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();

    if let Err(e) = a2a_nats_agent::run(EchoHandler::new()).await {
        error!(error = %e, "A2A echo agent exited with error");
        std::process::exit(1);
    }
}
