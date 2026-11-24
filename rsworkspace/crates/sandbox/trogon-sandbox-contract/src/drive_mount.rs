use serde::{Deserialize, Serialize};

use crate::{DriveId, DriveMountMode, MountPath};

/// A drive a sandbox mounts at `mount_path`.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct DriveMount {
    pub drive_id: DriveId,
    pub mount_path: MountPath,
    #[serde(default)]
    pub mode: DriveMountMode,
}
