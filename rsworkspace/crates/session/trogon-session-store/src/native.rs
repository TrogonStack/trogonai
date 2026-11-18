use async_nats::jetstream;
use trogon_decider_nats::{
    JetStreamStore, JetStreamStoreError, PublishStreamError, StreamStoreError, StreamSubject, StreamSubjectResolver,
    SubjectScope, SubjectScopeError, SubjectState, subject_current_position,
};
use trogon_nats::NatsToken;

use crate::{SessionAppend, SessionAppendFailure, SessionKey};

impl<R: StreamSubjectResolver<SessionKey>> SessionAppend for JetStreamStore<R> {
    fn classify_session_append(&self, error: &Self::Error) -> SessionAppendFailure {
        match error {
            JetStreamStoreError::OptimisticConcurrencyConflict(_) => SessionAppendFailure::WriteConflict,
            JetStreamStoreError::AppendStream(StreamStoreError::Publish(PublishStreamError::DuplicateEventId)) => {
                SessionAppendFailure::DuplicateIdentity
            }
            _ => SessionAppendFailure::Unknown,
        }
    }
}

#[derive(Clone, Debug)]
pub struct SessionStreamProfile {
    stream_name: String,
    namespace: NatsToken,
    minimum_replicas: usize,
    scope: SubjectScope,
}

impl SessionStreamProfile {
    pub fn new(
        stream_name: impl Into<String>,
        namespace: NatsToken,
        minimum_replicas: usize,
    ) -> Result<Self, SessionNativeError> {
        let scope = SubjectScope::new(format!("{namespace}.session.sessions"))?;
        Ok(Self {
            stream_name: stream_name.into(),
            namespace,
            minimum_replicas,
            scope,
        })
    }
}

#[derive(Clone, Debug)]
pub struct SessionSubjectResolver {
    profile: SessionStreamProfile,
}

impl SessionSubjectResolver {
    pub fn new(profile: SessionStreamProfile) -> Self {
        Self { profile }
    }
}

impl StreamSubjectResolver<SessionKey> for SessionSubjectResolver {
    type Error = SessionNativeError;

    fn subject_scope(&self) -> Option<&SubjectScope> {
        Some(&self.profile.scope)
    }

    async fn resolve_subject_state(
        &self,
        stream: &jetstream::stream::Stream,
        key: &SessionKey,
    ) -> Result<SubjectState, Self::Error> {
        let _ = (&self.profile.stream_name, &self.profile.namespace, self.profile.minimum_replicas);
        let subject = StreamSubject::new(key.subject())?;
        let current_position = subject_current_position(stream, &subject).await?;
        Ok(SubjectState { subject, current_position })
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SessionNativeError {
    #[error("invalid Session stream profile")]
    InvalidProfile,
    #[error("Session stream no longer meets the protected profile")]
    ProfileMismatch,
    #[error("Session key is outside the protected namespace")]
    NamespaceMismatch,
    #[error("cannot inspect the current Session stream profile")]
    Inspect(#[from] jetstream::stream::InfoError),
    #[error(transparent)]
    Scope(#[from] SubjectScopeError),
    #[error(transparent)]
    Subject(#[from] async_nats::SubjectError),
    #[error(transparent)]
    CurrentPosition(#[from] StreamStoreError),
}
