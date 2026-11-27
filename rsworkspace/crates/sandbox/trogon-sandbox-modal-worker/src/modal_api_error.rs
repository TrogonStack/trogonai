use std::path::StripPrefixError;

/// Modal, or the bridge that talks to it, refused or failed a call.
#[derive(Debug, thiserror::Error)]
pub enum ModalApiError {
    #[error("Modal returned {kind}: {message}")]
    Modal { kind: String, message: String },
    #[error("Modal sandbox not found: {0}")]
    SandboxNotFound(String),
    #[error("Modal sandbox not ready after {attempts} attempts")]
    SandboxNotReady { attempts: u32 },
    #[error("Modal authentication failed: {0}")]
    Auth(String),
    #[error("the bridge produced no output")]
    NoOutput,
    #[error("the bridge stdin is unavailable")]
    StdinUnavailable,
    #[error("failed to spawn {python}: {source}")]
    Spawn { python: String, source: std::io::Error },
    #[error("bridge op {op} exited with {status}: {stderr}")]
    Exited { op: String, status: String, stderr: String },
    #[error("the bridge exchange failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("the bridge spoke invalid json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("a workspace path escaped its root: {0}")]
    StripPrefix(#[from] StripPrefixError),
}
