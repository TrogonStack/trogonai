use crate::{SessionKeyError, SessionProjectKeyError, SessionStoreError};

#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    #[error(transparent)]
    Project(#[from] SessionProjectKeyError),
    #[error(transparent)]
    Key(#[from] SessionKeyError),
    #[error(transparent)]
    Store(#[from] SessionStoreError),
    #[error("session configuration directory could not be resolved")]
    MissingConfigDirectory,
    #[error("session configuration path is unsafe")]
    UnsafeConfigPath,
    #[error("session store load timed out")]
    LoadTimeout,
    #[error("session resume exceeds the configured entry limit")]
    EntryLimit,
    #[error("session resume exceeds the configured byte limit")]
    ByteLimit,
    #[error("session resume exceeds the configured subpath limit")]
    SubpathLimit,
    #[error("session materialization I/O failed")]
    Io(#[source] std::io::Error),
    #[error("session transcript serialization failed")]
    Encoding(#[source] serde_json::Error),
    #[error("transcript mirror flush timed out")]
    MirrorFlushTimeout,
    #[error("transcript mirror worker stopped")]
    MirrorStopped,
}
