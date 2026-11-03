use super::{HeartbeatError, state::EvolveError};
use trogonai_proto::peers::v1alpha1 as proto;
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum MessageError {
    #[error("assigned batch does not exist")]
    BatchNotFound,
    #[error("accepted message does not exist")]
    MessageNotFound,
    #[error("batch already acknowledged by application")]
    AlreadyAcknowledged(Box<proto::PeerBatch>),
    #[error("message already settled")]
    AlreadySettled(Box<proto::RecipientReceipt>),
    #[error("oldest prepared batch awaits application acknowledgment")]
    AlreadyPrepared(Box<proto::PeerBatch>),
    #[error("batch selection or rendered budget is invalid")]
    InvalidPlan,
    #[error("recipient mismatch")]
    RecipientMismatch,
    #[error("tenant mismatch")]
    TenantMismatch,
    #[error("recipient pending quota exhausted; intake deferred")]
    DeferIntake,
    #[error("recipient disposition is required")]
    RequiresDisposition,
    #[error("message already durably intaken")]
    AlreadyIntaken(Box<proto::InboxEntry>),
    #[error("message already disposed")]
    AlreadyDisposed(Box<proto::RecipientReceipt>),
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
