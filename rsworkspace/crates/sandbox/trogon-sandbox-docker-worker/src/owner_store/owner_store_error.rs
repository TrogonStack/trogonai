use std::path::PathBuf;

/// Why the owner identity could not be loaded or created.
#[derive(Debug, thiserror::Error)]
pub enum OwnerStoreError {
    #[error("create state dir {}", path.display())]
    CreateDir {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("another worker owns this state directory")]
    Locked(#[source] std::fs::TryLockError),
    #[error("read or write the owner record")]
    Io(#[from] std::io::Error),
}
