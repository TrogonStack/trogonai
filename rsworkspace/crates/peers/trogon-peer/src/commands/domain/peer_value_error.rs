#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("peer value construction is not implemented")]
pub struct PeerValueError;
