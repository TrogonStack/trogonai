use crate::FileNotFoundError;

/// Why reading a file failed: it does not exist, or the worker's own error.
#[derive(Debug, thiserror::Error)]
pub enum ReadFileError<E> {
    #[error(transparent)]
    NotFound(FileNotFoundError),
    #[error(transparent)]
    Other(E),
}
