use std::error::Error;
use std::time::Duration;

use grpc_nats_micro::client::RequestError;
use grpc_nats_micro::{ReplyError, ServiceError, ServiceErrorCode};
use trogonai_proto::secrets::exchange::v1alpha1::{ResolveFailure, ResolveFailureCategory as Category};

use super::{InvalidReleaseResponseError, ReleaseSubjectError, UnsealError};

/// Why a release call produced no value.
///
/// Every variant fails closed: nothing is released, and nothing a caller
/// could fall back to is implied. [`ReleaseError::Denied`] carries nothing,
/// as the secrets service answers every refusal the same way, so a requester
/// cannot tell an unknown credential from a refusing policy.
#[derive(Debug, thiserror::Error)]
pub enum ReleaseError {
    #[error("release denied")]
    Denied,
    #[error("release refused the requester xkey")]
    KeyDenied,
    #[error("secrets service is unavailable")]
    Unavailable,
    #[error("secrets service found the credential's integrity broken")]
    IntegrityFailure,
    #[error("secrets service throttled the release; retry after {retry_after:?}")]
    Throttled { retry_after: Duration },
    #[error("secrets service failed internally")]
    Internal,
    /// The secrets service refused the request itself on the ADR 0016 error
    /// channel, before judging the release.
    #[error("secrets service refused the release request as invalid")]
    InvalidArgument(#[source] ServiceError),
    /// The secrets service answered on the ADR 0016 error channel with a code
    /// other than `INVALID_ARGUMENT`.
    #[error("secrets service answered the release on its error channel")]
    ServiceFault(#[source] ServiceError),
    #[error("release call timed out")]
    Timeout,
    #[error("release call failed before an answer arrived")]
    Request(#[source] Box<dyn Error + Send + Sync>),
    #[error("release response is invalid")]
    InvalidResponse(#[from] InvalidReleaseResponseError),
    #[error("released value was already expired when it arrived")]
    Expired,
    /// The credential was invalidated, or the client lost track of its
    /// invalidations, while each release asked for it was on its way, so the
    /// value it carried may be the one revoked.
    #[error("release was invalidated while it was on its way")]
    Invalidated,
    #[error("released value did not open")]
    Unseal(#[from] UnsealError),
    #[error(transparent)]
    Subject(#[from] ReleaseSubjectError),
}

impl ReleaseError {
    /// Reads why the round trip produced no response.
    pub(crate) fn from_request<E>(error: RequestError<E>) -> Self
    where
        E: Error + Send + Sync + 'static,
    {
        match error {
            RequestError::Timeout { .. } => Self::Timeout,
            RequestError::Reply(ReplyError::Service(fault)) if fault.code() == ServiceErrorCode::INVALID_ARGUMENT => {
                Self::InvalidArgument(fault)
            }
            RequestError::Reply(ReplyError::Service(fault)) => Self::ServiceFault(fault),
            RequestError::Reply(reply) => InvalidReleaseResponseError::Reply(reply).into(),
            other => Self::Request(Box::new(other)),
        }
    }

    /// Reads the failure the secrets service answered, refusing one the
    /// contract does not allow on a release.
    pub(crate) fn reported(failure: ResolveFailure) -> Result<Self, InvalidReleaseResponseError> {
        let category = failure
            .category
            .as_known()
            .ok_or(InvalidReleaseResponseError::UnknownCategory(failure.category.to_i32()))?;
        match (category, failure.retry_after.into_option()) {
            (Category::RESOLVE_FAILURE_CATEGORY_THROTTLED, Some(retry_after)) => Ok(Self::Throttled {
                retry_after: Duration::try_from(retry_after).map_err(InvalidReleaseResponseError::RetryAfter)?,
            }),
            (Category::RESOLVE_FAILURE_CATEGORY_THROTTLED, None) => {
                Err(InvalidReleaseResponseError::ThrottledWithoutRetryAfter)
            }
            (_, Some(_)) => Err(InvalidReleaseResponseError::RetryAfterOutsideThrottled),
            (Category::RESOLVE_FAILURE_CATEGORY_DENIED, None) => Ok(Self::Denied),
            (Category::RESOLVE_FAILURE_CATEGORY_KEY_DENIED, None) => Ok(Self::KeyDenied),
            (Category::RESOLVE_FAILURE_CATEGORY_UNAVAILABLE, None) => Ok(Self::Unavailable),
            (Category::RESOLVE_FAILURE_CATEGORY_INTEGRITY_FAILURE, None) => Ok(Self::IntegrityFailure),
            (Category::RESOLVE_FAILURE_CATEGORY_INTERNAL, None) => Ok(Self::Internal),
            (
                Category::RESOLVE_FAILURE_CATEGORY_UNSPECIFIED
                | Category::RESOLVE_FAILURE_CATEGORY_RECONNECT_REQUIRED
                | Category::RESOLVE_FAILURE_CATEGORY_REFRESH_PENDING,
                None,
            ) => Err(InvalidReleaseResponseError::UnexpectedCategory(
                failure.category.to_i32(),
            )),
        }
    }
}

#[cfg(test)]
mod tests;
