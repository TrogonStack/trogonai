use a2a_nats::agent::{A2aError, A2aHandler, TaskEventStream};
use a2a_types::{
    Message, Part, Role, SendMessageResponse, StreamResponse, Task, TaskState, TaskStatus, TaskStatusUpdateEvent,
};
use tracing::error;

struct EchoHandler;

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
        let task_id = uuid::Uuid::new_v4().to_string();
        let context_id = if input.context_id.is_empty() {
            uuid::Uuid::new_v4().to_string()
        } else {
            input.context_id.clone()
        };

        let task = completed_task(&task_id, &context_id, &input);

        Ok(SendMessageResponse {
            payload: Some(a2a_types::send_message_response::Payload::Task(task)),
        })
    }

    async fn message_stream(&self, req: a2a_types::SendMessageRequest) -> Result<(Task, TaskEventStream), A2aError> {
        let input = req.message.unwrap_or_default();
        let task_id = uuid::Uuid::new_v4().to_string();
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

        let completed_task = completed_task(&task_id, &context_id, &input);
        let completed_event = StreamResponse {
            payload: Some(a2a_types::stream_response::Payload::Task(completed_task)),
        };

        let stream: TaskEventStream = Box::pin(futures::stream::iter([Ok(working_event), Ok(completed_event)]));

        Ok((initial, stream))
    }

    async fn tasks_get(&self, _req: a2a_types::GetTaskRequest) -> Result<Task, A2aError> {
        Err(A2aError::unsupported_operation("not implemented"))
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
        _req: a2a_types::TaskPushNotificationConfig,
    ) -> Result<a2a_types::TaskPushNotificationConfig, A2aError> {
        Err(A2aError::push_notification_not_supported("not implemented"))
    }

    async fn push_notification_get(
        &self,
        _req: a2a_types::GetTaskPushNotificationConfigRequest,
    ) -> Result<a2a_types::TaskPushNotificationConfig, A2aError> {
        Err(A2aError::push_notification_not_supported("not implemented"))
    }

    async fn push_notification_list(
        &self,
        _req: a2a_types::ListTaskPushNotificationConfigsRequest,
    ) -> Result<a2a_types::ListTaskPushNotificationConfigsResponse, A2aError> {
        Err(A2aError::push_notification_not_supported("not implemented"))
    }

    async fn push_notification_delete(
        &self,
        _req: a2a_types::DeleteTaskPushNotificationConfigRequest,
    ) -> Result<(), A2aError> {
        Err(A2aError::push_notification_not_supported("not implemented"))
    }

    async fn agent_card(&self, _req: a2a_types::GetExtendedAgentCardRequest) -> Result<a2a_types::AgentCard, A2aError> {
        Err(A2aError::unsupported_operation("not implemented"))
    }
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();

    if let Err(e) = a2a_nats_agent::run(EchoHandler).await {
        error!(error = %e, "A2A agent exited with error");
        std::process::exit(1);
    }
}
