use std::convert::Infallible;

use trogon_decider_runtime::{CommandError, PrincipalIdError};
use trogon_peer::{EnrollError, EvolveError, PeerValueError};
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
    #[error("peer enrollment service is not implemented")]
    NotImplemented,
    #[error("invalid peer enrollment field {field}: {source}")]
    InvalidInput {
        field: EnrollmentField,
        #[source]
        source: PeerValueError,
    },
    #[error("trusted peer owner cannot form a command principal: {0}")]
    Principal(#[from] PrincipalIdError),
    #[error("peer enrollment execution failed: {0}")]
    Execution(#[source] EnrollExecutionError<ReadError, AppendError>),
}

pub type EnrollResult<ReadError, AppendError> =
    Result<v1alpha1::EnrollResponse, EnrollServiceError<ReadError, AppendError>>;
