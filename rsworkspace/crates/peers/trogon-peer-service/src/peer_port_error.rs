#[derive(Debug, thiserror::Error)]
pub enum PeerPortError {
    #[error("peer port is temporarily unavailable")]
    Unavailable,
    #[error("peer port denied the scoped operation")]
    Denied,
    #[error("peer port integrity check failed")]
    Integrity,
    #[error("immutable peer port record conflicts")]
    Conflict,
    #[error("peer backend failed: {0}")]
    Backend(#[source] Box<dyn std::error::Error + Send + Sync>),
}
