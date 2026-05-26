use std::collections::HashMap;
use std::sync::Mutex;

use a2a_nats::agent::{A2aError, A2aHandler, TaskEventStream};
use a2a_types::{
    GetTaskRequest, Message, Part, Role, SendMessageResponse, StreamResponse, Task, TaskPushNotificationConfig,
    TaskState, TaskStatus, TaskStatusUpdateEvent,
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
        content: Some(a2a_types::part::Content::Text(text.into())),
        ..Default::default()
    }
}

fn agent_message(task_id: &str, context_id: &str, text: impl Into<String>) -> Message {
    Message {
        message_id: uuid::Uuid::new_v4().to_string(),
        task_id: task_id.to_owned(),
        context_id: context_id.to_owned(),
        role: Role::Agent as i32,
        parts: vec![text_part(text)],
        ..Default::default()
    }
}

fn completed_task(task_id: &str, context_id: &str, input: &Message) -> Task {
    let echo_text = input
        .parts
        .first()
        .and_then(|p| {
            if let Some(a2a_types::part::Content::Text(t)) = &p.content {
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
        status: Some(TaskStatus {
            state: TaskState::Completed as i32,
            ..Default::default()
        }),
        history: vec![input.clone(), reply],
        ..Default::default()
    }
}

#[async_trait::async_trait]
impl A2aHandler for EchoHandler {
    async fn message_send(&self, req: a2a_types::SendMessageRequest) -> Result<SendMessageResponse, A2aError> {
        let input = req.message.unwrap_or_default();
        let task_id = task_id_from_message_id(&input.message_id);
        let context_id = if input.context_id.is_empty() {
            uuid::Uuid::new_v4().to_string()
        } else {
            input.context_id.clone()
        };

        let task = completed_task(&task_id, &context_id, &input);
        self.store_task(task.clone());

        Ok(SendMessageResponse {
            payload: Some(a2a_types::send_message_response::Payload::Task(task)),
        })
    }

    async fn message_stream(&self, req: a2a_types::SendMessageRequest) -> Result<(Task, TaskEventStream), A2aError> {
        let input = req.message.unwrap_or_default();
        let task_id = task_id_from_message_id(&input.message_id);
        let context_id = if input.context_id.is_empty() {
            uuid::Uuid::new_v4().to_string()
        } else {
            input.context_id.clone()
        };

        let initial = Task {
            id: task_id.clone(),
            context_id: context_id.clone(),
            status: Some(TaskStatus {
                state: TaskState::Submitted as i32,
                ..Default::default()
            }),
            ..Default::default()
        };

        let working_event = StreamResponse {
            payload: Some(a2a_types::stream_response::Payload::StatusUpdate(
                TaskStatusUpdateEvent {
                    task_id: task_id.clone(),
                    context_id: context_id.clone(),
                    status: Some(TaskStatus {
                        state: TaskState::Working as i32,
                        ..Default::default()
                    }),
                    ..Default::default()
                },
            )),
        };

        let completed = completed_task(&task_id, &context_id, &input);
        self.store_task(completed.clone());
        let completed_event = StreamResponse {
            payload: Some(a2a_types::stream_response::Payload::Task(completed)),
        };

        let stream: TaskEventStream = Box::pin(futures::stream::iter([Ok(working_event), Ok(completed_event)]));

        Ok((initial, stream))
    }

    async fn tasks_get(&self, req: GetTaskRequest) -> Result<Task, A2aError> {
        let task_id = req.id;
        let guard = self.tasks.lock().map_err(|_| A2aError::internal("task store lock poisoned"))?;
        guard
            .get(&task_id)
            .cloned()
            .ok_or_else(|| A2aError::task_not_found(&task_id))
    }

    async fn tasks_list(&self, _req: a2a_types::ListTasksRequest) -> Result<a2a_types::ListTasksResponse, A2aError> {
        Err(A2aError::unsupported_operation("not implemented"))
    }

    async fn tasks_cancel(&self, _req: a2a_types::CancelTaskRequest) -> Result<Task, A2aError> {
        Err(A2aError::unsupported_operation("not implemented"))
    }

    async fn tasks_resubscribe(&self, _req: a2a_types::SubscribeToTaskRequest) -> Result<Task, A2aError> {
        Err(A2aError::unsupported_operation("not implemented"))
    }

    async fn push_notification_set(
        &self,
        req: a2a_types::TaskPushNotificationConfig,
    ) -> Result<a2a_types::TaskPushNotificationConfig, A2aError> {
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
        req: a2a_types::GetTaskPushNotificationConfigRequest,
    ) -> Result<a2a_types::TaskPushNotificationConfig, A2aError> {
        let guard = self
            .push_configs
            .lock()
            .map_err(|_| A2aError::internal("push config store lock poisoned"))?;
        guard
            .get(&req.task_id)
            .and_then(|bucket| bucket.iter().find(|c| c.id == req.id).cloned())
            .ok_or_else(|| A2aError::push_notification_not_supported("config not found"))
    }

    async fn push_notification_list(
        &self,
        req: a2a_types::ListTaskPushNotificationConfigsRequest,
    ) -> Result<a2a_types::ListTaskPushNotificationConfigsResponse, A2aError> {
        let guard = self
            .push_configs
            .lock()
            .map_err(|_| A2aError::internal("push config store lock poisoned"))?;
        let configs = guard.get(&req.task_id).cloned().unwrap_or_default();
        Ok(a2a_types::ListTaskPushNotificationConfigsResponse {
            configs,
            next_page_token: String::new(),
        })
    }

    async fn push_notification_delete(
        &self,
        req: a2a_types::DeleteTaskPushNotificationConfigRequest,
    ) -> Result<(), A2aError> {
        let mut guard = self
            .push_configs
            .lock()
            .map_err(|_| A2aError::internal("push config store lock poisoned"))?;
        if let Some(bucket) = guard.get_mut(&req.task_id) {
            bucket.retain(|c| c.id != req.id);
        }
        Ok(())
    }

    async fn agent_card(&self, _req: a2a_types::GetExtendedAgentCardRequest) -> Result<a2a_types::AgentCard, A2aError> {
        Err(A2aError::unsupported_operation("not implemented"))
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
