use std::convert::Infallible;

use crate::execution_error::ExecutionError;
use crate::history_error::HistoryError;
use trogon_decider_runtime::{CommandError, PrincipalIdError};
use trogon_peer::{AttachError, EnrollError, EvolveError, PeerValueError};
use trogonai_proto::peers::{PeerEventPayloadError, v1alpha1};

use crate::EnrollmentField;

pub type EnrollExecutionError<ReadError, AppendError> = CommandError<
    EnrollError,
    EvolveError,
    Infallible,
    ReadError,
    AppendError,
    PeerEventPayloadError,
    PeerEventPayloadError,
    PeerEventPayloadError,
>;

#[derive(Debug, thiserror::Error)]
pub enum EnrollServiceError<ReadError, AppendError> {
    #[error("peer resume attachment reference is invalid: {0}")]
    ResumeReference(#[source] PeerValueError),

    #[error("peer attachment changed during resume: {0}")]
    ResumeGuard(#[source] trogon_peer::HeartbeatError),

    #[error("peer enrollment is outside the injected store tenant scope")]
    Scope,
    #[error("peer attachment changed during resume; retry enrollment")]
    ResumeChanged,
    #[error("peer enrollment field {field} is missing")]
    MissingInput { field: EnrollmentField },
    #[error("invalid peer enrollment field {field}: {source}")]
    InvalidInput {
        field: EnrollmentField,
        #[source]
        source: PeerValueError,
    },
    #[error("trusted peer owner cannot form a command principal: {0}")]
    Principal(#[from] PrincipalIdError),
    #[error("peer attachment execution failed: {0}")]
    Attach(#[source] ExecutionError<AttachError, ReadError, AppendError>),
    #[error("peer history read failed: {0}")]
    History(#[source] HistoryError<ReadError>),
    #[error("peer enrollment execution failed: {0}")]
    Execution(#[source] EnrollExecutionError<ReadError, AppendError>),
}

pub type EnrollResult<ReadError, AppendError> =
    Result<v1alpha1::EnrollResponse, EnrollServiceError<ReadError, AppendError>>;
