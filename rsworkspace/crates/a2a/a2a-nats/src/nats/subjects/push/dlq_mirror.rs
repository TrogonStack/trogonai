use trogon_identity_types::{SubjectPrincipal, SubjectPrincipalError};

use super::dead_letter_tokens::DeadLetterTokens;
use crate::a2a_prefix::A2aPrefix;
use crate::task_id::{A2aTaskId, TaskIdError};

/// `{prefix}.v1.push.dlq.mirror.{kind}.{id}.{task_id}`: the copy of a dead letter
/// the gateway republishes so a tenant can read its own failures without reading
/// the authoritative records.
///
/// It keeps the principal position of the dead letter it copies (ADR#0082), so a
/// reader filtered on its own principal sees only its own failures. The `mirror`
/// token gives it one more token than an authoritative record, so neither lane's
/// stream filter captures the other's subjects.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PushDlqMirrorSubject {
    prefix: A2aPrefix,
    caller: SubjectPrincipal,
    task_id: A2aTaskId,
}

impl PushDlqMirrorSubject {
    pub fn new(prefix: &A2aPrefix, caller: &SubjectPrincipal, task_id: &A2aTaskId) -> Self {
        Self {
            prefix: prefix.clone(),
            caller: caller.clone(),
            task_id: task_id.clone(),
        }
    }

    pub fn parse(prefix: &A2aPrefix, subject: &str) -> Result<Self, PushDlqMirrorSubjectError> {
        let tail = subject
            .strip_prefix(prefix.as_str())
            .and_then(|rest| rest.strip_prefix(".v1.push.dlq.mirror."))
            .ok_or(PushDlqMirrorSubjectError::NotPushDlqMirror)?;
        let tokens = DeadLetterTokens::split(tail).ok_or(PushDlqMirrorSubjectError::BadSubjectShape)?;
        let caller =
            SubjectPrincipal::from_tokens(tokens.kind, tokens.id).map_err(PushDlqMirrorSubjectError::InvalidCaller)?;
        let task_id = A2aTaskId::new(tokens.task_id).map_err(PushDlqMirrorSubjectError::InvalidTaskId)?;
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
}

impl std::fmt::Display for PushDlqMirrorSubject {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}.v1.push.dlq.mirror.{}.{}",
            self.prefix.as_str(),
            self.caller,
            self.task_id.as_str()
        )
    }
}

impl async_nats::subject::ToSubject for PushDlqMirrorSubject {
    fn to_subject(&self) -> async_nats::subject::Subject {
        async_nats::subject::Subject::from(self.to_string().as_str())
    }
}

impl super::super::markers::Publishable for PushDlqMirrorSubject {}

impl super::super::stream::StreamAssignment for PushDlqMirrorSubject {
    const STREAM: Option<super::super::stream::A2aStream> = Some(super::super::stream::A2aStream::PushDlq);
}

/// Why a subject is not a mirrored push dead letter under the configured prefix.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum PushDlqMirrorSubjectError {
    #[error("subject does not start with '{{prefix}}.v1.push.dlq.mirror.' for the configured prefix")]
    NotPushDlqMirror,
    #[error("expected '{{prefix}}.v1.push.dlq.mirror.{{kind}}.{{id}}.{{task_id}}'")]
    BadSubjectShape,
    #[error("caller principal tokens are invalid")]
    InvalidCaller(#[source] SubjectPrincipalError),
    #[error("task id token is invalid")]
    InvalidTaskId(#[source] TaskIdError),
}

#[cfg(test)]
mod tests;
