use crate::FileNotFound;

/// Why reading a file failed: it does not exist, or the worker's own error.
#[derive(Debug, thiserror::Error)]
pub enum ReadFileFailure<E> {
    #[error(transparent)]
    NotFound(FileNotFound),
    #[error(transparent)]
    Other(E),
}
