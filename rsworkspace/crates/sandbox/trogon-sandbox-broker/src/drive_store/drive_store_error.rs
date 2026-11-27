use std::fmt;

#[derive(Debug, thiserror::Error)]
pub enum DriveStoreError {
    #[error("drive not found")]
    NotFound,
    #[error("drive already exists")]
    AlreadyExists,
    #[error("revision conflict: expected revision does not match current value")]
    RevisionConflict,
    #[error("drive already attached to another lease")]
    AlreadyAttached,
    #[error("drive store backend error: {0}")]
    Backend(String),
}

impl DriveStoreError {
    pub(super) fn backend(message: impl fmt::Display) -> Self {
        Self::Backend(message.to_string())
    }
}
