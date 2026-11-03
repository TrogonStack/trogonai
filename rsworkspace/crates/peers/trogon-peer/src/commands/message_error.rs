use super::{state::EvolveError, HeartbeatError};
use trogonai_proto::peers::v1alpha1 as proto;
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum MessageError {
    #[error("invalid message transition")]
    InvalidTransition,
    #[error("invalid state: {0}")]
    InvalidState(#[from] EvolveError),
    #[error("attachment guard: {0}")]
    Guard(#[from] HeartbeatError),
    #[error("invalid value: {0}")]
    Value(#[from] super::domain::PeerValueError),
    #[error("semantic retry conflicts")]
    Conflict,
    #[error("sender pending capacity exhausted")]
    SenderCapacity,
    #[error("sender new acceptance rate exhausted")]
    RateLimited,
    #[error("collaboration mismatch")]
    CollaborationMismatch,
    #[error("message already accepted")]
    AlreadyAccepted(Box<proto::OutboxEntry>),
}
