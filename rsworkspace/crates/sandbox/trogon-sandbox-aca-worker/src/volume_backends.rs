use trogon_sandbox_volumes::{StoreResourceName, VolumeBackend};

use crate::VolumeCredential;

/// Chooses the object store a volume is synced with.
pub trait VolumeBackends: Send + Sync {
    fn backend(
        &self,
        container: StoreResourceName,
        account: StoreResourceName,
        credential: &VolumeCredential,
    ) -> VolumeBackend;
}
