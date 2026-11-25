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
    /// A store call exceeded the load timeout. The text is upstream's, e.g. `SessionStore.load() timed out after 60000ms
    /// for session <id>`.
    #[error("{0}")]
    LoadTimeout(String),
    /// A store call failed while a resume was materialized under the Python dialect, worded like Python's
    /// `_with_timeout`: `<call> failed during resume materialization: <error>`, where `<error>` is the backend's own
    /// message.
    #[error("{call} failed during resume materialization: {}", python_str(source))]
    ResumeStore {
        call: String,
        #[source]
        source: SessionStoreError,
    },
    #[error("CLAUDE_CODE_CUSTOM_OAUTH_URL is not an approved endpoint.")]
    UnapprovedOauthEndpoint,
    #[error("Options.continue with sessionStore requires store.listSessions to be implemented")]
    ContinueRequiresList,
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

/// `str(e)` of the exception a Python store raises: the backend's own message, and the empty text of the
/// `NotImplementedError` an optional method the store leaves out raises.
fn python_str(error: &SessionStoreError) -> String {
    match error {
        SessionStoreError::Backend(source) => source.to_string(),
        SessionStoreError::Unsupported(_) => String::new(),
        other => other.to_string(),
    }
}
