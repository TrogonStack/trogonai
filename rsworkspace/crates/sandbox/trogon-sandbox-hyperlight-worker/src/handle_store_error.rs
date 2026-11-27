use trogon_sandbox_protocol::AuthorityResourceIdError;

/// Why the retained handle store refused an operation.
#[derive(Debug, thiserror::Error)]
pub enum HandleStoreError {
    /// The lease already retains a native handle.
    #[error("lease still retains a native handle")]
    Retained,
    /// The handle is retiring and its cleanup has not been confirmed.
    #[error("native cleanup has not confirmed completion")]
    CleanupUnconfirmed,
    /// Another holder still borrows the handle.
    #[error("native handle is still borrowed")]
    Borrowed,
    /// The resource identifier is not valid.
    #[error(transparent)]
    Resource(#[from] AuthorityResourceIdError),
}
