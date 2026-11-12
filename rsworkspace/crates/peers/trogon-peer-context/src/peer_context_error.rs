#[derive(Debug, thiserror::Error)]
pub enum PeerContextError {
    #[error("prepared peer assignment violates the context contract")]
    ContractViolation,
    #[error("peer context could not be encoded")]
    Encode(#[source] serde_json::Error),
}
