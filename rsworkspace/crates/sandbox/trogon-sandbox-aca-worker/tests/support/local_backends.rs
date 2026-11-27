use std::path::PathBuf;

use trogon_sandbox_aca_worker::{VolumeBackends, VolumeCredential};
use trogon_sandbox_volumes::{StoreResourceName, VolumeBackend};

/// Syncs volumes with a directory instead of Azure Blob Storage.
pub struct LocalBackends {
    pub root: PathBuf,
}

impl VolumeBackends for LocalBackends {
    fn backend(
        &self,
        _container: StoreResourceName,
        _account: StoreResourceName,
        _credential: &VolumeCredential,
    ) -> VolumeBackend {
        VolumeBackend::LocalFs {
            root: self.root.clone(),
        }
    }
}
