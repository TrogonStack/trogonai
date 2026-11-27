use std::path::PathBuf;

/// Why the API key file could not supply a key.
#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    #[error("API key file {path} is not a file")]
    NotAFile { path: PathBuf },
    #[error("failed to read API key file {path}: {source}")]
    ReadFile {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("API key file {path} must have mode 0600 (found {mode:o})")]
    InsecureFileMode { path: PathBuf, mode: u32 },
    #[error("API key file {path} is empty")]
    Empty { path: PathBuf },
}
