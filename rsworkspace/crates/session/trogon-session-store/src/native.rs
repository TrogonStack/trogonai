use async_nats::jetstream;
use trogon_decider_nats::{
    StreamStoreError, StreamSubject, StreamSubjectResolver, SubjectScope, SubjectScopeError, SubjectState,
    subject_current_position,
};
use trogon_nats::NatsToken;

use crate::SessionKey;

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
        let stream_name = stream_name.into();
        if !(1..=5).contains(&minimum_replicas)
            || stream_name.is_empty()
            || stream_name.len() > 128
            || !stream_name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
        {
            return Err(SessionNativeError::InvalidProfile);
        }
        let scope = SubjectScope::new(format!("{namespace}.session.sessions"))?;
        Ok(Self {
            stream_name,
            namespace,
            minimum_replicas,
            scope,
        })
    }

    fn validate(&self, actual: &jetstream::stream::Config) -> Result<(), SessionNativeError> {
        use jetstream::stream::{DiscardPolicy, PersistenceMode, RetentionPolicy, StorageType};

        if actual.name != self.stream_name
            || actual.subjects.as_slice() != [self.scope.pattern()]
            || actual.storage != StorageType::File
            || actual.retention != RetentionPolicy::Limits
            || actual.discard != DiscardPolicy::New
            || actual.max_bytes <= 0
            || actual.max_messages <= 0
            || !(1..=262144).contains(&actual.max_message_size)
            || actual.max_messages_per_subject > 0
            || actual.discard_new_per_subject
            || !actual.max_age.is_zero()
            || actual.allow_message_ttl
            || actual.subject_delete_marker_ttl.is_some()
            || !actual.deny_delete
            || !actual.deny_purge
            || actual.allow_rollup
            || !actual.allow_atomic_publish
            || actual.no_ack
            || actual.sealed
            || actual.num_replicas < self.minimum_replicas
            || actual.allow_message_schedules
            || actual.allow_message_counter
            || actual.persist_mode == Some(PersistenceMode::Async)
            || actual.mirror.is_some()
            || actual.sources.as_ref().is_some_and(|sources| !sources.is_empty())
            || actual.subject_transform.is_some()
            || actual.republish.is_some()
            || !actual.template_owner.is_empty()
        {
            return Err(SessionNativeError::ProfileMismatch);
        }
        Ok(())
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
        if key.namespace() != &self.profile.namespace {
            return Err(SessionNativeError::NamespaceMismatch);
        }
        // Configuration can change while this handle's cached metadata stays unchanged.
        self.profile.validate(&stream.get_info().await?.config)?;
        let subject = StreamSubject::new(key.subject())?;
        let current_position = subject_current_position(stream, &subject).await?;
        Ok(SubjectState {
            subject,
            current_position,
        })
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
