use async_nats::jetstream::stream::Stream;
use trogon_credential_domain::{CredentialKey, CredentialOwnerId};
use trogon_decider_nats::{StreamSubject, StreamSubjectResolver, SubjectScope, SubjectState, subject_current_position};
use trogon_nats::NatsToken;

use crate::CredentialEventSubjectResolverError;
use crate::constants::CREDENTIAL_EVENT_SUBJECT_PREFIX;

/// Resolves one project's credential streams onto its event subjects.
///
/// ADR#0027 makes the resolver the isolation boundary, so each project gets a
/// resolver whose [`SubjectScope`] is its own subtree,
/// `credentials.events.v1.{project}`. The opaque credential id sits
/// directly below it, which keeps an aggregate from being addressable outside
/// its project even though the id itself carries no hierarchy.
#[derive(Clone, Debug)]
pub struct CredentialEventSubjectResolver {
    owner_id: CredentialOwnerId,
    scope: SubjectScope,
}

impl CredentialEventSubjectResolver {
    pub fn for_owner(owner_id: &CredentialOwnerId) -> Result<Self, CredentialEventSubjectResolverError> {
        let token = credential_project_token(owner_id)?;
        let scope = SubjectScope::new(format!("{CREDENTIAL_EVENT_SUBJECT_PREFIX}.{token}"))
            .map_err(|source| CredentialEventSubjectResolverError::BuildScope { source })?;
        Ok(Self {
            owner_id: owner_id.clone(),
            scope,
        })
    }

    /// The subject carrying one credential's events.
    pub fn subject_for(&self, stream_id: &CredentialKey) -> Result<StreamSubject, CredentialEventSubjectResolverError> {
        if stream_id.owner_id() != &self.owner_id {
            return Err(CredentialEventSubjectResolverError::OutsideProject {
                owner_id: self.owner_id.clone(),
                stream_id: stream_id.clone(),
            });
        }
        StreamSubject::new(format!("{}{}", self.scope.as_prefix(), stream_id.credential_id()))
            .map_err(|source| CredentialEventSubjectResolverError::ValidateSubject { source })
    }

    pub fn scope(&self) -> &SubjectScope {
        &self.scope
    }
}

/// The subject for one credential stream, whichever project it belongs to.
pub fn credential_event_subject(
    stream_id: &CredentialKey,
) -> Result<StreamSubject, CredentialEventSubjectResolverError> {
    CredentialEventSubjectResolver::for_owner(stream_id.owner_id())?.subject_for(stream_id)
}

/// The project token of a credential event subject, and of any other subject
/// that names the credential's project.
///
/// Owner ids may carry dots, which would split the token, and never carry
/// `~`, so the substitution cannot make two projects share a subtree.
pub fn credential_project_token(
    owner_id: &CredentialOwnerId,
) -> Result<NatsToken, CredentialEventSubjectResolverError> {
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

#[cfg(test)]
mod tests;
