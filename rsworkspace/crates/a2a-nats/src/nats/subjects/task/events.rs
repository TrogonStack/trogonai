use crate::a2a_prefix::A2aPrefix;
use crate::req_id::ReqId;
use crate::task_id::A2aTaskId;

/// `{prefix}.task.{task_id}.events.{req_id}` — JetStream-backed task event subject.
///
/// Published by the agent for `message/stream` and `tasks/resubscribe`. The `req_id`
/// suffix lets a single task fan out to multiple concurrent subscribers without
/// cross-talk.
#[derive(Debug)]
pub struct TaskEventsSubject {
    prefix: A2aPrefix,
    task_id: A2aTaskId,
    req_id: ReqId,
}

impl TaskEventsSubject {
    pub fn new(prefix: &A2aPrefix, task_id: &A2aTaskId, req_id: &ReqId) -> Self {
        Self {
            prefix: prefix.clone(),
            task_id: task_id.clone(),
            req_id: req_id.clone(),
        }
    }
}

impl std::fmt::Display for TaskEventsSubject {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}.task.{}.events.{}",
            self.prefix.as_str(),
            self.task_id.as_str(),
            self.req_id.as_str()
        )
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
