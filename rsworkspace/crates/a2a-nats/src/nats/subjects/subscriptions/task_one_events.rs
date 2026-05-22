use crate::a2a_prefix::A2aPrefix;
use crate::task_id::A2aTaskId;

/// `{prefix}.task.{task_id}.events.*` — JetStream filter subject for all event streams
/// of a single task, regardless of `req_id`. Used by `tasks/resubscribe` consumers.
#[derive(Debug)]
pub struct TaskOneEventsSubject {
    prefix: A2aPrefix,
    task_id: A2aTaskId,
}

impl TaskOneEventsSubject {
    pub fn new(prefix: &A2aPrefix, task_id: &A2aTaskId) -> Self {
        Self {
            prefix: prefix.clone(),
            task_id: task_id.clone(),
        }
    }
}

impl std::fmt::Display for TaskOneEventsSubject {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.task.{}.events.*", self.prefix.as_str(), self.task_id.as_str())
    }
}

impl async_nats::subject::ToSubject for TaskOneEventsSubject {
    fn to_subject(&self) -> async_nats::subject::Subject {
        async_nats::subject::Subject::from(self.to_string().as_str())
    }
}

impl super::super::markers::Subscribable for TaskOneEventsSubject {}

impl super::super::stream::StreamAssignment for TaskOneEventsSubject {
    const STREAM: Option<super::super::stream::A2aStream> = None;
}
