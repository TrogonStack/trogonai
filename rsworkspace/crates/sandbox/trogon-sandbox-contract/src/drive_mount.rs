use serde::{Deserialize, Serialize};

use crate::{DriveError, DriveId, DriveMountMode};

/// A drive a sandbox mounts at `mount_path`.
///
/// Deserialization does not validate the path; [`DriveMount::validate`] does.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct DriveMount {
    pub drive_id: DriveId,
    pub mount_path: String,
    #[serde(default)]
    pub mode: DriveMountMode,
}

impl DriveMount {
    /// Requires an absolute mount path that does not resolve to `/` (such as `/.` or `//`) and has no
    /// `..` component.
    pub fn validate(&self) -> Result<(), DriveError> {
        let reason = if !self.mount_path.starts_with('/') {
            "must be absolute"
        } else if self
            .mount_path
            .split('/')
            .all(|component| component.is_empty() || component == ".")
        {
            "must not be '/'"
        } else if self.mount_path.split('/').any(|component| component == "..") {
            "must not contain '..' components"
        } else {
            return Ok(());
        };
        Err(DriveError::InvalidDriveMountPath {
            mount_path: self.mount_path.clone(),
            reason: reason.to_string(),
        })
    }
}
