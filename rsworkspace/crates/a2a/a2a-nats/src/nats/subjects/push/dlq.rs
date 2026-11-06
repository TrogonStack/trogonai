use trogon_identity_types::{SubjectPrincipal, SubjectPrincipalError};

use super::dead_letter_tokens::DeadLetterTokens;
use super::dlq_mirror::PushDlqMirrorSubject;
use crate::a2a_prefix::A2aPrefix;
use crate::task_id::{A2aTaskId, TaskIdError};

/// `{prefix}.v1.push.dlq.{kind}.{id}.{task_id}`: the authoritative record of a push
/// delivery that failed for good, captured by the `PUSH_DLQ` stream.
///
/// `{kind}.{id}` is the principal of the caller the task ran for, taken from the
/// subject its request was delivered on (ADR#0082), so a dead letter is addressed
/// to the caller the bus admitted and never to a name a header carried.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PushDlqSubject {
    prefix: A2aPrefix,
    caller: SubjectPrincipal,
    task_id: A2aTaskId,
}

impl PushDlqSubject {
    pub fn new(prefix: &A2aPrefix, caller: &SubjectPrincipal, task_id: &A2aTaskId) -> Self {
        Self {
            prefix: prefix.clone(),
            caller: caller.clone(),
            task_id: task_id.clone(),
        }
    }

    pub fn parse(prefix: &A2aPrefix, subject: &str) -> Result<Self, PushDlqSubjectError> {
        let tail = subject
            .strip_prefix(prefix.as_str())
            .and_then(|rest| rest.strip_prefix(".v1.push.dlq."))
            .ok_or(PushDlqSubjectError::NotPushDlq)?;
        let tokens = DeadLetterTokens::split(tail).ok_or(PushDlqSubjectError::BadSubjectShape)?;
        let caller =
            SubjectPrincipal::from_tokens(tokens.kind, tokens.id).map_err(PushDlqSubjectError::InvalidCaller)?;
        let task_id = A2aTaskId::new(tokens.task_id).map_err(PushDlqSubjectError::InvalidTaskId)?;
        Ok(Self {
            prefix: prefix.clone(),
            caller,
            task_id,
        })
    }

    pub fn caller(&self) -> &SubjectPrincipal {
        &self.caller
    }

    pub fn task_id(&self) -> &A2aTaskId {
        &self.task_id
    }

    /// The copy of this dead letter a tenant reads, under the same principal.
    pub fn mirror(&self) -> PushDlqMirrorSubject {
        PushDlqMirrorSubject::new(&self.prefix, &self.caller, &self.task_id)
    }
}

impl std::fmt::Display for PushDlqSubject {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}.v1.push.dlq.{}.{}",
            self.prefix.as_str(),
            self.caller,
            self.task_id.as_str()
        )
    }
}

impl async_nats::subject::ToSubject for PushDlqSubject {
    fn to_subject(&self) -> async_nats::subject::Subject {
        async_nats::subject::Subject::from(self.to_string().as_str())
    }
}

impl super::super::markers::Publishable for PushDlqSubject {}

impl super::super::stream::StreamAssignment for PushDlqSubject {
    const STREAM: Option<super::super::stream::A2aStream> = Some(super::super::stream::A2aStream::PushDlq);
}

/// Why a subject is not an authoritative push dead letter under the configured prefix.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum PushDlqSubjectError {
    #[error("subject does not start with '{{prefix}}.v1.push.dlq.' for the configured prefix")]
    NotPushDlq,
    #[error("expected '{{prefix}}.v1.push.dlq.{{kind}}.{{id}}.{{task_id}}'")]
    BadSubjectShape,
    #[error("caller principal tokens are invalid")]
    InvalidCaller(#[source] SubjectPrincipalError),
    #[error("task id token is invalid")]
    InvalidTaskId(#[source] TaskIdError),
}

#[cfg(test)]
mod tests;
