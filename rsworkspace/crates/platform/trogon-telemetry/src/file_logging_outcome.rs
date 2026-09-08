use std::path::PathBuf;

/// How preparing the on-disk log file turned out.
///
/// Kept as data rather than as a pre-formatted message so the callsite records
/// the path and the failure as `tracing` fields, which a subscriber can filter
/// and forward, instead of interpolating them into message text.
pub(crate) enum FileLoggingOutcome {
    Enabled { path: PathBuf },
    DirectoryUnavailable { error: anyhow::Error },
    FileUnavailable { path: PathBuf, error: std::io::Error },
}

impl FileLoggingOutcome {
    pub(crate) fn record(&self) {
        match self {
            Self::Enabled { path } => {
                tracing::info!(log_file = %path.display(), "file logging enabled");
            }
            Self::DirectoryUnavailable { error } => {
                tracing::info!(error = %error, "file logging disabled: log directory unavailable");
            }
            Self::FileUnavailable { path, error } => {
                let log_file = path.display();
                tracing::info!(%log_file, %error, "file logging disabled: log file could not be opened");
            }
        }
    }
}
