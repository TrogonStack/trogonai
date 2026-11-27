use std::sync::{Arc, Mutex};
use std::time::Duration;

use trogon_sandbox_contract::{FileContentHashing, FileRecursion};
use trogon_sandbox_file_ops::{FileOpError, Manifest, ManifestSource};

use crate::MemoryFs;
use crate::mutex_ext::MutexExt;

/// The lease's file map seen as a manifest source.
pub(crate) struct MemorySource {
    pub fs: Arc<Mutex<MemoryFs>>,
}

impl ManifestSource for MemorySource {
    async fn manifest(
        &self,
        path: &str,
        recursion: FileRecursion,
        hashing: FileContentHashing,
    ) -> Result<Manifest, FileOpError> {
        self.fs.lock_clean().manifest(path, recursion, hashing)
    }

    fn scan_interval(&self) -> Duration {
        Duration::from_millis(20)
    }
}
