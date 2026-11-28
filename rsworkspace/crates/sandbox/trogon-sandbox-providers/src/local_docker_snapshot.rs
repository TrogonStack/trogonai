use crate::{CheckpointCreateSpec, CheckpointRestoreSpec, ContainerRunSpec, DockerError, LocalDockerProvider};

impl LocalDockerProvider {
    /// Commits a running container and exports its root filesystem as an uncompressed tar.
    ///
    /// Layer splitting (base vs installed-tooling) is not implemented: bollard exposes
    /// commit/export per container but does not provide image-layer diff without pulling
    /// the image graph separately. Callers gzip the tar before OCI push.
    pub async fn commit_and_export_rootfs(&self, container_id: &str) -> Result<Vec<u8>, DockerError> {
        let tag = format!("snap-{}", uuid::Uuid::new_v4());
        let repo = "trogon-sandbox-snapshot";
        self.engine
            .commit(container_id, repo, &tag)
            .await
            .map_err(|error| error.during("docker commit failed"))?;
        self.engine
            .export_rootfs(container_id)
            .await
            .map_err(|error| error.during("docker export failed"))
    }

    /// Imports a root filesystem tar (plain tar, not docker save format) as a runnable image.
    pub async fn import_rootfs_image(&self, tar: &[u8], image_ref: &str) -> Result<String, DockerError> {
        let (repo, tag) = split_image_ref(image_ref);
        self.engine
            .import_rootfs(tar, repo, tag)
            .await
            .map_err(|error| error.during("docker import failed"))
    }

    /// CRIU checkpoint of a running container without stopping it (`exit=false`).
    pub async fn checkpoint_running(
        &self,
        container_id: &str,
        checkpoint_name: &str,
        checkpoint_dir: &str,
    ) -> Result<(), DockerError> {
        self.engine
            .create_checkpoint_leave_running(
                container_id,
                &CheckpointCreateSpec {
                    checkpoint_id: checkpoint_name.to_string(),
                    checkpoint_dir: checkpoint_dir.to_string(),
                },
            )
            .await
            .map_err(|error| error.during("docker checkpoint create failed"))
    }

    /// Restore a stopped container from checkpoint files previously exported to ORAS.
    pub async fn restore_from_checkpoint(
        &self,
        run_spec: ContainerRunSpec,
        checkpoint_name: &str,
        checkpoint_dir: &str,
    ) -> Result<String, DockerError> {
        let container_id = self
            .engine
            .create_container_stopped(run_spec)
            .await
            .map_err(|error| error.during("create container for checkpoint restore failed"))?;
        self.engine
            .start_from_checkpoint(
                &container_id,
                &CheckpointRestoreSpec {
                    checkpoint_id: checkpoint_name.to_string(),
                    checkpoint_dir: checkpoint_dir.to_string(),
                },
            )
            .await
            .map_err(|error| error.during("docker start --checkpoint failed"))?;
        Ok(container_id)
    }
}

/// Splits `repo[:tag]`. A colon before the last `/` is a registry port, not a tag separator.
fn split_image_ref(image_ref: &str) -> (&str, &str) {
    match image_ref.rsplit_once(':') {
        Some((repo, tag)) if !tag.contains('/') => (repo, tag),
        _ => (image_ref, "latest"),
    }
}

#[cfg(test)]
mod tests;
