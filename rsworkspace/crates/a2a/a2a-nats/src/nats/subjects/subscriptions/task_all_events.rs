use crate::a2a_prefix::A2aPrefix;

/// `{prefix}.v1.tasks.*.events`: filter subject covering every task's events.
///
/// The source of the `EVENTS` stream, and the filter for consumers that cannot narrow
/// to one task: the gateway durable, which serves every caller, and the gateway's
/// `message/stream` pump, which never sees the bootstrap reply that names the task.
#[derive(Debug)]
pub struct TaskAllEventsSubject {
    prefix: A2aPrefix,
}

impl TaskAllEventsSubject {
    pub fn new(prefix: &A2aPrefix) -> Self {
        Self { prefix: prefix.clone() }
    }
}

impl std::fmt::Display for TaskAllEventsSubject {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.v1.tasks.*.events", self.prefix.as_str())
    }
}

impl async_nats::subject::ToSubject for TaskAllEventsSubject {
    fn to_subject(&self) -> async_nats::subject::Subject {
        async_nats::subject::Subject::from(self.to_string().as_str())
    }
}

impl super::super::markers::Subscribable for TaskAllEventsSubject {}

impl super::super::stream::StreamAssignment for TaskAllEventsSubject {
    const STREAM: Option<super::super::stream::A2aStream> = None;
}

#[cfg(test)]
mod tests;
