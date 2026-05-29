use thiserror::Error;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ChainExplorerError {
    #[error("root event not found")]
    RootNotFound,
    #[error("orphaned event: no parent for request_id {request_id}")]
    OrphanedEvent { request_id: String },
    #[error("malformed chain for request_id {request_id}: {reason}")]
    MalformedChain { request_id: String, reason: String },
}
