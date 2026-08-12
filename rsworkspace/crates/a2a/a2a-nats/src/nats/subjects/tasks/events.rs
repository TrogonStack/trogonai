use crate::a2a_prefix::A2aPrefix;
use crate::task_id::A2aTaskId;

/// `{prefix}.v1.tasks.{task_id}.events`: JetStream-backed task event subject.
///
/// Published by the agent for `message/stream` and `tasks/resubscribe`. Scoped to
/// the task, not the request (ADR#0055). Concurrent subscribers of the same task
/// are told apart by the `Trogon-Req-Id` header, which every event carries.
#[derive(Debug)]
pub struct TaskEventsSubject {
    prefix: A2aPrefix,
    task_id: A2aTaskId,
}

impl TaskEventsSubject {
    pub fn new(prefix: &A2aPrefix, task_id: &A2aTaskId) -> Self {
        Self {
            prefix: prefix.clone(),
            task_id: task_id.clone(),
        }
    }
}

impl std::fmt::Display for TaskEventsSubject {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.v1.tasks.{}.events", self.prefix.as_str(), self.task_id.as_str())
    }
}

impl async_nats::subject::ToSubject for TaskEventsSubject {
    fn to_subject(&self) -> async_nats::subject::Subject {
        async_nats::subject::Subject::from(self.to_string().as_str())
    }
}

impl super::super::markers::Publishable for TaskEventsSubject {}
impl super::super::markers::JetStreamEvents for TaskEventsSubject {}

impl super::super::stream::StreamAssignment for TaskEventsSubject {
    const STREAM: Option<super::super::stream::A2aStream> = Some(super::super::stream::A2aStream::Events);
}

#[cfg(test)]
mod tests;
