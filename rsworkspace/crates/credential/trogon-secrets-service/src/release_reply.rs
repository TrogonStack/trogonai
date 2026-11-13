use buffa::MessageField;
use buffa_types::google::protobuf::{Duration, Timestamp};
use grpc_nats_micro::{ContentType, ServiceFault};
use trogonai_proto::secrets::exchange::v1alpha1::{
    ResolveFailure, ResolveFailureCategory, ResolveForReleaseResponse, SealedAccess,
};

use super::constants::THROTTLED_RETRY_AFTER;
use super::{CredentialReleaseError, SealedRelease};

/// What the release endpoint answers a call it judged, typed as the exchange
/// contract types it.
///
/// Each [`CredentialReleaseError`] answers its own category and nothing else,
/// so a denial carries no more than the release core let it.
pub(crate) struct ReleaseReply(ResolveForReleaseResponse);

impl ReleaseReply {
    pub(crate) fn of(result: Result<SealedRelease, CredentialReleaseError>) -> Self {
        let result = match result {
            Ok(release) => SealedAccess {
                xkey_sealed_value: release.sealed_value().as_bytes().to_vec(),
                bundle_generation: release.bundle_generation(),
                valid_until: MessageField::some(Timestamp::from(release.valid_until())),
            }
            .into(),
            Err(error) => ResolveFailure {
                category: category(error).into(),
                retry_after: MessageField::none(),
            }
            .into(),
        };
        Self(ResolveForReleaseResponse { result })
    }

    /// The answer to a call the endpoint has no room for, which asks the
    /// caller to retry after [`THROTTLED_RETRY_AFTER`].
    pub(crate) fn throttled() -> Self {
        Self(ResolveForReleaseResponse {
            result: ResolveFailure {
                category: ResolveFailureCategory::RESOLVE_FAILURE_CATEGORY_THROTTLED.into(),
                retry_after: MessageField::some(Duration::from(THROTTLED_RETRY_AFTER)),
            }
            .into(),
        })
    }

    pub(crate) fn encode(&self, content_type: ContentType) -> Result<Vec<u8>, ServiceFault> {
        content_type
            .encode(&self.0)
            .map_err(|error| ServiceFault::internal(format!("release reply did not encode: {error}")))
    }
}

fn category(error: CredentialReleaseError) -> ResolveFailureCategory {
    match error {
        CredentialReleaseError::Denied => ResolveFailureCategory::RESOLVE_FAILURE_CATEGORY_DENIED,
        CredentialReleaseError::KeyDenied => ResolveFailureCategory::RESOLVE_FAILURE_CATEGORY_KEY_DENIED,
        CredentialReleaseError::Unavailable => ResolveFailureCategory::RESOLVE_FAILURE_CATEGORY_UNAVAILABLE,
        CredentialReleaseError::IntegrityFailure => ResolveFailureCategory::RESOLVE_FAILURE_CATEGORY_INTEGRITY_FAILURE,
        CredentialReleaseError::Internal => ResolveFailureCategory::RESOLVE_FAILURE_CATEGORY_INTERNAL,
    }
}

#[cfg(test)]
mod tests;
