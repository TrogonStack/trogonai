use std::path::PathBuf;
use std::sync::Arc;

#[cfg(target_os = "linux")]
use super::BubblewrapBackend;
#[cfg(target_os = "macos")]
use super::SeatbeltBackend;
use super::{IsolationBackend, PassthroughBackend};

/// Builds the backend that confines payloads under `kind`.
pub fn build_isolation_backend(
    kind: crate::config::IsolationBackendKind,
    bwrap_path: PathBuf,
) -> anyhow::Result<Arc<dyn IsolationBackend>> {
    match kind {
        #[cfg(target_os = "linux")]
        crate::config::IsolationBackendKind::Bubblewrap => {
            super::bubblewrap_version::BubblewrapVersion::warn_if_outdated(&bwrap_path);
            Ok(Arc::new(BubblewrapBackend::new(bwrap_path)))
        }
        #[cfg(target_os = "macos")]
        crate::config::IsolationBackendKind::Seatbelt => {
            let _ = bwrap_path;
            Ok(Arc::new(SeatbeltBackend::new()))
        }
        crate::config::IsolationBackendKind::None => Ok(Arc::new(PassthroughBackend::default())),
    }
}
