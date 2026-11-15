use async_nats::SubjectError;
use async_nats::jetstream::stream;
use async_nats::jetstream::stream::Stream;
use trogon_credential_domain::{CredentialKey, CredentialOwnerId};
use trogon_decider_nats::{
    StreamStoreError, StreamSubject, StreamSubjectResolver, SubjectScope, SubjectScopeError, SubjectState,
    subject_current_position,
};
use trogon_nats::{NatsToken, SubjectTokenViolationError};

use super::constants::{CREDENTIAL_EVENT_SUBJECT_PREFIX, CREDENTIAL_STREAM};
use super::event_stream_mismatch_error::CredentialEventStreamMismatchError;

/// Resolves one project's credential streams onto its event subjects.
///
/// ADR#0027 makes the resolver the isolation boundary, so each project gets a
/// resolver whose [`SubjectScope`] is its own subtree,
/// `credentials.events.v1.{project}`. The opaque credential id sits
/// directly below it, which keeps an aggregate from being addressable outside
/// its project even though the id itself carries no hierarchy.
#[derive(Clone, Debug)]
pub(crate) struct CredentialEventSubjectResolver {
    owner_id: CredentialOwnerId,
    scope: SubjectScope,
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum CredentialEventSubjectResolverError {
    #[error("failed to build credential event subject token: {source}")]
    BuildSubjectToken {
        #[source]
        source: SubjectTokenViolationError,
    },
    #[error("failed to build the credential event subject scope: {source}")]
    BuildScope {
        #[source]
        source: SubjectScopeError,
    },
    #[error("failed to validate credential event subject: {source}")]
    ValidateSubject {
        #[source]
        source: SubjectError,
    },
    #[error("credential stream '{stream_id}' does not belong to project '{owner_id}'")]
    OutsideProject {
        owner_id: CredentialOwnerId,
        stream_id: CredentialKey,
    },
    #[error(transparent)]
    Stream(StreamStoreError),
}

impl CredentialEventSubjectResolver {
    pub(crate) fn for_owner(owner_id: &CredentialOwnerId) -> Result<Self, CredentialEventSubjectResolverError> {
        let token = owner_subject_token(owner_id)?;
        let scope = SubjectScope::new(format!("{CREDENTIAL_EVENT_SUBJECT_PREFIX}.{token}"))
            .map_err(|source| CredentialEventSubjectResolverError::BuildScope { source })?;
        Ok(Self {
            owner_id: owner_id.clone(),
            scope,
        })
    }

    /// The subject carrying one credential's events.
    pub(crate) fn subject_for(
        &self,
        stream_id: &CredentialKey,
    ) -> Result<StreamSubject, CredentialEventSubjectResolverError> {
        if stream_id.owner_id() != &self.owner_id {
            return Err(CredentialEventSubjectResolverError::OutsideProject {
                owner_id: self.owner_id.clone(),
                stream_id: stream_id.clone(),
            });
        }
        StreamSubject::new(format!("{}{}", self.scope.as_prefix(), stream_id.credential_id()))
            .map_err(|source| CredentialEventSubjectResolverError::ValidateSubject { source })
    }

    pub(crate) fn scope(&self) -> &SubjectScope {
        &self.scope
    }
}

/// The subject for one credential stream, whichever project it belongs to.
pub(crate) fn credential_event_subject(
    stream_id: &CredentialKey,
) -> Result<StreamSubject, CredentialEventSubjectResolverError> {
    CredentialEventSubjectResolver::for_owner(stream_id.owner_id())?.subject_for(stream_id)
}

/// The project token of a credential event subject.
///
/// Owner ids may carry dots, which would split the token, and never carry
/// `~`, so the substitution cannot make two projects share a subtree.
fn owner_subject_token(owner_id: &CredentialOwnerId) -> Result<NatsToken, CredentialEventSubjectResolverError> {
    NatsToken::new(owner_id.as_str().replace('.', "~"))
        .map_err(|source| CredentialEventSubjectResolverError::BuildSubjectToken { source })
}

impl StreamSubjectResolver<CredentialKey> for CredentialEventSubjectResolver {
    type Error = CredentialEventSubjectResolverError;

    fn subject_scope(&self) -> Option<&SubjectScope> {
        Some(&self.scope)
    }

    async fn resolve_subject_state(
        &self,
        events_stream: &Stream,
        stream_id: &CredentialKey,
    ) -> Result<SubjectState, Self::Error> {
        let subject = self.subject_for(stream_id)?;
        let current_position = subject_current_position(events_stream, &subject)
            .await
            .map_err(CredentialEventSubjectResolverError::Stream)?;

        Ok(SubjectState {
            subject,
            current_position,
        })
    }
}

pub(crate) fn stream_config() -> stream::Config {
    stream::Config {
        name: CREDENTIAL_STREAM.to_string(),
        subjects: vec![format!("{CREDENTIAL_EVENT_SUBJECT_PREFIX}.>")],
        allow_atomic_publish: true,
        ..Default::default()
    }
}

/// Refuses an event stream that could drop events on its own.
///
/// The credential streams are the system of record: replay rebuilds every
/// identity from the write request that opened its stream, so a stream that
/// forgets events on restart, discards them once consumed, ages them out, caps
/// them, or lets a publish erase them would lose identities that nothing else
/// records.
pub(crate) fn validate_stream_config(actual: &stream::Config) -> Result<(), CredentialEventStreamMismatchError> {
    if actual.storage != stream::StorageType::File {
        return Err(CredentialEventStreamMismatchError::NotOnDisk);
    }
    if actual.retention != stream::RetentionPolicy::Limits {
        return Err(CredentialEventStreamMismatchError::DiscardsConsumedEvents);
    }
    if !actual.max_age.is_zero() {
        return Err(CredentialEventStreamMismatchError::ExpiresByAge);
    }
    if actual.max_messages_per_subject > 0 {
        return Err(CredentialEventStreamMismatchError::CapsEventsPerSubject);
    }
    if actual.max_messages > 0 {
        return Err(CredentialEventStreamMismatchError::CapsEvents);
    }
    if actual.max_bytes > 0 {
        return Err(CredentialEventStreamMismatchError::CapsBytes);
    }
    if actual.allow_message_ttl {
        return Err(CredentialEventStreamMismatchError::ExpiresPerEvent);
    }
    if actual.allow_rollup {
        return Err(CredentialEventStreamMismatchError::RollsUpSubjects);
    }
    Ok(())
}

#[cfg(test)]
mod tests;
