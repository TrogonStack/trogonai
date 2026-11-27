use std::path::Path;
use std::time::Instant;

use tokio::fs;
use tokio::process::Command;
use trogon_sandbox_contract::{HostMountAccess, RemoteMountSource};

use crate::{
    FuseMetrics, FuseMountConfig, FuseMountHandle, FusePlatform, VolumeError, build_mount_args,
    parse_proc_mounts_for_fuse,
};

/// Mounts remote object-store prefixes into host directories through `mount-s3`.
pub struct FuseMountManager {
    config: FuseMountConfig,
    metrics: FuseMetrics,
}

impl FuseMountManager {
    pub fn new(config: FuseMountConfig, metrics: FuseMetrics) -> Self {
        Self { config, metrics }
    }

    pub async fn mount(
        &self,
        source: &RemoteMountSource,
        host_dir: &Path,
        access: HostMountAccess,
    ) -> Result<FuseMountHandle, VolumeError> {
        if self.config.platform == FusePlatform::Unsupported {
            return Err(VolumeError::FuseUnsupported);
        }
        fs::create_dir_all(host_dir).await?;

        let RemoteMountSource::S3 { bucket, .. } = source;
        let args = build_mount_args(&self.config, source, host_dir, access);

        let started_at = Instant::now();
        let output = Command::new(&self.config.mount_s3_binary)
            .args(&args)
            .output()
            .await
            .map_err(VolumeError::FuseSpawnFailed)?;

        if !output.status.success() {
            return Err(VolumeError::FuseMountRejected {
                status: output.status,
                stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            });
        }

        self.wait_for_mount(host_dir).await?;

        let label = bucket.as_str();
        self.metrics
            .fuse_mount_duration_seconds
            .with_label_values(&[label])
            .observe(started_at.elapsed().as_secs_f64());
        self.metrics.fuse_mount_total.with_label_values(&[label]).inc();

        Ok(FuseMountHandle {
            host_dir: host_dir.to_owned(),
            unmount_binary: self.config.unmount_binary.clone(),
            fallback_unmount_binary: self.config.fallback_unmount_binary.clone(),
        })
    }

    async fn wait_for_mount(&self, host_dir: &Path) -> Result<(), VolumeError> {
        let deadline = Instant::now() + self.config.ready_timeout;
        loop {
            if let Ok(contents) = fs::read_to_string(&self.config.mount_table).await
                && parse_proc_mounts_for_fuse(&contents, host_dir)
            {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(VolumeError::FuseMountTimeout {
                    path: host_dir.to_owned(),
                });
            }
            tokio::time::sleep(self.config.ready_poll_interval).await;
        }
    }
}
