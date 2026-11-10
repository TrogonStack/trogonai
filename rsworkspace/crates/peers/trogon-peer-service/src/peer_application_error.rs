use crate::{execution_error::ExecutionError, history_error::HistoryError, peer_port_error::PeerPortError};
use trogon_decider_runtime::{AuthorizationDeniedError, PrincipalIdError};
use trogon_peer::{DetachError, HeartbeatError, MessageError, PeerObservationError, PeerValueError};
#[derive(Debug, thiserror::Error)]
pub enum PeerApplicationError<ReadError, AppendError> {
    #[error("peer application operation is not implemented")]
    NotImplemented,
    #[error("invalid or missing peer request field")]
    Input,
    #[error("peer request value is invalid: {0}")]
    Value(#[from] PeerValueError),
    #[error("peer observation is invalid: {0}")]
    Observation(#[from] PeerObservationError),
    #[error("peer owner cannot form a principal: {0}")]
    Principal(#[from] PrincipalIdError),
    #[error("peer operation denied: {0}")]
    Denied(#[from] AuthorizationDeniedError),
    #[error("peer history failed: {0}")]
    History(#[from] HistoryError<ReadError>),
    #[error("peer lifecycle execution history failed: {0}")]
    LifetimeHistory(#[source] HistoryError<ReadError>),
    #[error("peer port failed: {0}")]
    Port(#[from] PeerPortError),
    #[error("peer heartbeat failed: {0}")]
    Heartbeat(#[source] ExecutionError<HeartbeatError, ReadError, AppendError>),
    #[error("peer detach failed: {0}")]
    Detach(#[source] ExecutionError<DetachError, ReadError, AppendError>),
    #[error("peer message execution failed: {0}")]
    Message(#[source] ExecutionError<MessageError, ReadError, AppendError>),
    #[error("peer evidence is invalid: {0}")]
    Evidence(#[from] MessageError),
    #[error("peer request is outside its trusted scope")]
    Scope,
}
