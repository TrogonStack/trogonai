use serde::{Deserialize, Serialize};

/// Why a worker could not export a snapshot, tagged by `kind`.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize, thiserror::Error)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum SnapshotExportError {
    #[error("snapshot export unsupported: {message}")]
    Unsupported { message: String },
    #[error("snapshot export failed transiently: {message}")]
    Transient { message: String },
    #[error("snapshot export failed permanently: {message}")]
    Permanent { message: String },
}
