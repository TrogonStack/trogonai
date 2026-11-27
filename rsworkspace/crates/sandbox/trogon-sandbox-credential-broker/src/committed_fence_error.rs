use crate::PolicyStoreError;

/// A lease's committed policy could not be turned into a fence.
#[derive(Debug, thiserror::Error)]
pub enum CommittedFenceError {
    #[error("reading the committed credential policy failed")]
    Unavailable(#[source] PolicyStoreError),
    #[error("no committed credential policy exists for this lease and the worker requires one")]
    NotCommitted,
}
