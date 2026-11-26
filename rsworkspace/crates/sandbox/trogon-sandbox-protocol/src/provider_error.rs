use crate::native_options_error::NativeOptionsError;
use crate::privileged_unsupported_error::PrivilegedUnsupportedError;
use crate::snapshot_error::SnapshotError;

/// Why a [`Provider`](crate::Provider) operation failed.
#[derive(Debug, thiserror::Error)]
pub enum ProviderError {
    /// The lease's vendor-specific options were refused.
    #[error(transparent)]
    NativeOptions(#[from] NativeOptionsError),
    /// The lease asked for elevated capabilities the provider cannot grant.
    #[error(transparent)]
    PrivilegedUnsupported(#[from] PrivilegedUnsupportedError),
    /// A snapshot could not be taken.
    #[error(transparent)]
    Snapshot(#[from] SnapshotError),
    /// The backing platform failed.
    #[error("{0}")]
    Backend(Box<dyn std::error::Error + Send + Sync>),
}

impl ProviderError {
    /// A failure of the backing platform, carrying its own error.
    pub fn backend(error: impl Into<Box<dyn std::error::Error + Send + Sync>>) -> Self {
        Self::Backend(error.into())
    }
}
