use std::path::PathBuf;

use tokio::fs;
use tokio::process::Command;

use crate::VolumeError;

/// A live FUSE mount that can be unmounted.
pub struct FuseMountHandle {
    pub host_dir: PathBuf,
    pub(crate) unmount_binary: PathBuf,
    pub(crate) fallback_unmount_binary: PathBuf,
}

impl FuseMountHandle {
    /// Unmounts the directory and removes it.
    pub async fn unmount(self) -> Result<(), VolumeError> {
        let primary = Command::new(&self.unmount_binary)
            .arg("-u")
            .arg(&self.host_dir)
            .output()
            .await;
        let unmounted = match primary {
            Ok(out) if out.status.success() => true,
            _ => {
                let fallback = Command::new(&self.fallback_unmount_binary)
                    .arg(&self.host_dir)
                    .output()
                    .await;
                matches!(fallback, Ok(out) if out.status.success())
            }
        };

        if !unmounted {
            return Err(VolumeError::FuseUnmountFailed { path: self.host_dir });
        }

        fs::remove_dir_all(&self.host_dir).await?;
        Ok(())
    }
}
