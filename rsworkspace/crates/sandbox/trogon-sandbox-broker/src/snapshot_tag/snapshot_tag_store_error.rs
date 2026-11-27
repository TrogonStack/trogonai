use std::fmt;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum SnapshotTagStoreError {
    #[error("snapshot tag not found")]
    NotFound,
    #[error("revision conflict: expected revision does not match current value")]
    RevisionConflict,
    #[error("snapshot tag store backend error: {0}")]
    Backend(String),
}

impl SnapshotTagStoreError {
    pub(super) fn backend(message: impl fmt::Display) -> Self {
        Self::Backend(message.to_string())
    }
}
