use crate::{PeerIngressError, PeerReplySubjectError};

#[derive(Debug, thiserror::Error)]
pub enum PeerRequestError {
    #[error("peer requester is not implemented")]
    NotImplemented,
    #[error("peer request route is invalid: {0}")]
    Ingress(#[from] PeerIngressError),
    #[error("peer response destination is invalid: {0}")]
    Reply(#[from] PeerReplySubjectError),
    #[error("peer request transport failed: {0}")]
    Transport(#[from] async_nats::RequestError),
}
