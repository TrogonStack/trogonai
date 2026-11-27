use crate::{AuditError, PolicyStoreError, TokenStoreError};

/// The broker's NATS-backed stores could not be opened.
#[derive(Debug, thiserror::Error)]
pub enum BrokerSetupError {
    #[error("token bucket could not be opened")]
    Tokens(#[from] TokenStoreError),
    #[error("audit stream could not be opened")]
    Audit(#[from] AuditError),
    #[error("committed policy bucket could not be opened")]
    Policies(#[from] PolicyStoreError),
}
