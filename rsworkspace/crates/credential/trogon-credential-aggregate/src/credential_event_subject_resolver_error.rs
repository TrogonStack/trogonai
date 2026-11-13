use async_nats::SubjectError;
use trogon_credential_domain::{CredentialKey, CredentialOwnerId};
use trogon_decider_nats::{StreamStoreError, SubjectScopeError};
use trogon_nats::SubjectTokenViolationError;

#[derive(Debug, thiserror::Error)]
pub enum CredentialEventSubjectResolverError {
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
