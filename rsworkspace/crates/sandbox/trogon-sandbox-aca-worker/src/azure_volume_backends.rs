use trogon_sandbox_volumes::{StoreResourceName, VolumeBackend};

use crate::{VolumeBackends, VolumeCredential};

/// Syncs volumes with Azure Blob Storage.
///
/// The volumes crate builds its Azure store without a credential parameter, so the issued
/// credential is checked but not handed to the store.
#[derive(Debug, Clone, Copy, Default)]
pub struct AzureVolumeBackends;

impl VolumeBackends for AzureVolumeBackends {
    fn backend(
        &self,
        container: StoreResourceName,
        account: StoreResourceName,
        _credential: &VolumeCredential,
    ) -> VolumeBackend {
        VolumeBackend::AzureBlob { container, account }
    }
}
