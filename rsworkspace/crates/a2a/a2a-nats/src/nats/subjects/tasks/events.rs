use trogon_identity_types::{SubjectPrincipal, SubjectPrincipalError};

use super::super::routed::{RoutedSplitError, RoutedTokens};
use crate::a2a_prefix::A2aPrefix;
use crate::task_id::{A2aTaskId, TaskIdError};

/// `{prefix}.v1.tasks.{task_id}.{kind}.{id}.events`: JetStream-backed task event subject.
///
/// Published by the agent for `message/stream` and `tasks/resubscribe`. Scoped to
/// the task, not the request (ADR#0055). Concurrent subscribers of the same task
/// are told apart by the `Trogon-Req-Id` header, which every event carries.
///
/// `{kind}.{id}` is the principal of the caller the task runs for, taken from the
/// delivery subject of the request that produced the event (ADR#0082), so a
/// consumer filtered on its own principal sees only its own tasks and the
/// gateway egresses each event to the caller this subject names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskEventsSubject {
    prefix: A2aPrefix,
    task_id: A2aTaskId,
    caller: SubjectPrincipal,
}

impl TaskEventsSubject {
    pub fn new(prefix: &A2aPrefix, task_id: &A2aTaskId, caller: &SubjectPrincipal) -> Self {
        Self {
            prefix: prefix.clone(),
            task_id: task_id.clone(),
            caller: caller.clone(),
        }
    }

    pub fn parse(prefix: &A2aPrefix, subject: &str) -> Result<Self, TaskEventsSubjectError> {
        let tokens = RoutedTokens::split(prefix, "tasks", subject).map_err(|error| match error {
            RoutedSplitError::ForeignFamily => TaskEventsSubjectError::NotTaskEvents,
            RoutedSplitError::MissingTokens => TaskEventsSubjectError::BadSubjectShape,
        })?;
        if tokens.terminal != "events" {
            return Err(TaskEventsSubjectError::BadSubjectShape);
        }
        let task_id = A2aTaskId::new(tokens.route).map_err(TaskEventsSubjectError::InvalidTaskId)?;
        let caller =
            SubjectPrincipal::from_tokens(tokens.kind, tokens.id).map_err(TaskEventsSubjectError::InvalidCaller)?;
        Ok(Self {
            prefix: prefix.clone(),
            task_id,
            caller,
        })
    }

    pub fn task_id(&self) -> &A2aTaskId {
        &self.task_id
    }

    pub fn caller(&self) -> &SubjectPrincipal {
        &self.caller
    }
}

impl std::fmt::Display for TaskEventsSubject {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}.v1.tasks.{}.{}.events",
            self.prefix.as_str(),
            self.task_id.as_str(),
            self.caller
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

/// Why a subject is not a task event subject under the configured prefix.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum TaskEventsSubjectError {
    #[error("subject does not start with '{{prefix}}.v1.tasks.' for the configured prefix")]
    NotTaskEvents,
    #[error("expected '{{prefix}}.v1.tasks.{{task_id}}.{{kind}}.{{id}}.events'")]
    BadSubjectShape,
    #[error("task id token is invalid")]
    InvalidTaskId(#[source] TaskIdError),
    #[error("caller principal tokens are invalid")]
    InvalidCaller(#[source] SubjectPrincipalError),
}

#[cfg(test)]
mod tests;
