use trogonai_proto::convert::TimestampConversionError;

use super::domain::PeerValueError;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum EvolveError {
    #[error("invalid message history transition")]
    InvalidMessageTransition,
    #[error("peer event has no event case")]
    MissingEvent,
    #[error("peer event case is not supported by the enrollment fold")]
    UnsupportedEvent,
    #[error("peer enrollment already exists")]
    AlreadyEnrolled,
    #[error("peer attachment already exists")]
    AlreadyAttached,
    #[error("peer enrollment is missing")]
    MissingEnrollment,
    #[error("peer field '{field}' is missing")]
    MissingField { field: &'static str },
    #[error("peer field '{field}' is invalid: {source}")]
    InvalidValue {
        field: &'static str,
        #[source]
        source: PeerValueError,
    },
    #[error("peer timestamp '{field}' is invalid: {source}")]
    InvalidTimestamp {
        field: &'static str,
        #[source]
        source: TimestampConversionError,
    },
    #[error("peer timestamp '{field}' is outside the protobuf range")]
    TimestampRange { field: &'static str },
    #[error("peer identity does not match its enrollment tuple")]
    IdentityMismatch,
    #[error("peer attachment generation, status, or lease is invalid")]
    InvalidAttachment,
    #[error("peer heartbeat observation is invalid")]
    InvalidObservation,
    #[error("peer observation value is invalid: {0}")]
    InvalidObservationValue(#[source] super::domain::PeerObservationError),
}
