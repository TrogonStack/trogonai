use std::sync::Arc;

use buffa_types::{DurationError, TimestampError};
use grpc_nats_micro::ReplyError;

/// Why a reply from the secrets service is not a release response the
/// contract allows. Each one is refused: no part of such a reply is trusted.
#[derive(Clone, Debug, thiserror::Error)]
pub enum InvalidReleaseResponseError {
    #[error("release reply did not decode")]
    Reply(#[source] Arc<ReplyError>),
    #[error("release response carries neither an access nor a failure")]
    MissingResult,
    #[error("release failure category {0} is not one the contract defines")]
    UnknownCategory(i32),
    #[error("release failure category {0} is not one a release answers")]
    UnexpectedCategory(i32),
    #[error("throttled release failure carries no retry_after")]
    ThrottledWithoutRetryAfter,
    #[error("release failure carries a retry_after outside THROTTLED")]
    RetryAfterOutsideThrottled,
    #[error("release failure retry_after is not a non-negative duration")]
    RetryAfter(#[source] DurationError),
    #[error("sealed access carries no sealed value")]
    EmptySealedValue,
    #[error("sealed access carries no valid_until")]
    MissingValidUntil,
    #[error("sealed access valid_until is not after the epoch")]
    ValidUntilNotPositive,
    #[error("sealed access valid_until is not a valid timestamp")]
    ValidUntil(#[source] TimestampError),
}
