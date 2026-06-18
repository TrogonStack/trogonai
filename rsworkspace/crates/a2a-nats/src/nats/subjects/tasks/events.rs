use crate::a2a_prefix::A2aPrefix;
use crate::req_id::ReqId;
use crate::task_id::A2aTaskId;

/// `{prefix}.tasks.{task_id}.events.{req_id}` — JetStream-backed task event subject.
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
            "{}.tasks.{}.events.{}",
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_prefix_tasks_events_subject_with_req_id_suffix() {
        let s = TaskEventsSubject::new(
            &A2aPrefix::new("a2a").unwrap(),
            &A2aTaskId::new("task-1").unwrap(),
            &ReqId::from_test("r1"),
        );
        assert_eq!(s.to_string(), "a2a.tasks.task-1.events.r1");
    }

    #[test]
    fn to_subject_round_trips_display_form() {
        use async_nats::subject::ToSubject;
        let s = TaskEventsSubject::new(
            &A2aPrefix::new("a2a").unwrap(),
            &A2aTaskId::new("task-1").unwrap(),
            &ReqId::from_test("r1"),
        );
        assert_eq!(s.to_subject().as_str(), "a2a.tasks.task-1.events.r1");
    }
}
