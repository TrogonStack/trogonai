use trogon_sandbox_contract::{DriveId, MountPath, ProviderVolumeId};

use crate::{FlyVolumeBindingError, FlyVolumeId};

/// The one operator-provided Fly volume a worker offers, with the drive it serves and where it is mounted.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct FlyVolumeBinding {
    volume_id: FlyVolumeId,
    provider_volume_id: ProviderVolumeId,
    drive_id: DriveId,
    mount_path: MountPath,
}

impl FlyVolumeBinding {
    pub fn new(
        volume_id: FlyVolumeId,
        drive_id: DriveId,
        mount_path: MountPath,
    ) -> Result<Self, FlyVolumeBindingError> {
        let provider_volume_id = ProviderVolumeId::new(volume_id.as_str())?;
        Ok(Self {
            volume_id,
            provider_volume_id,
            drive_id,
            mount_path,
        })
    }

    pub fn volume_id(&self) -> &FlyVolumeId {
        &self.volume_id
    }

    pub fn provider_volume_id(&self) -> &ProviderVolumeId {
        &self.provider_volume_id
    }

    pub fn drive_id(&self) -> &DriveId {
        &self.drive_id
    }

    pub fn mount_path(&self) -> &MountPath {
        &self.mount_path
    }
}
