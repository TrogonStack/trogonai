use crate::a2a_prefix::A2aPrefix;
use crate::task_id::A2aTaskId;

/// `{prefix}.tasks.{task_id}.events.*` — JetStream filter subject for all event streams
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
        write!(f, "{}.tasks.{}.events.*", self.prefix.as_str(), self.task_id.as_str())
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_per_task_events_filter() {
        let s = TaskOneEventsSubject::new(&A2aPrefix::new("a2a").unwrap(), &A2aTaskId::new("task-7").unwrap());
        assert_eq!(s.to_string(), "a2a.tasks.task-7.events.*");
    }

    #[test]
    fn to_subject_round_trips_display_form() {
        use async_nats::subject::ToSubject;
        let s = TaskOneEventsSubject::new(&A2aPrefix::new("a2a").unwrap(), &A2aTaskId::new("task-7").unwrap());
        assert_eq!(s.to_subject().as_str(), "a2a.tasks.task-7.events.*");
    }
}
