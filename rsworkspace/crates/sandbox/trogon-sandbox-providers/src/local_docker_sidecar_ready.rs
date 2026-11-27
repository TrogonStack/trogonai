use std::path::{Path, PathBuf};

use crate::DockerError;

/// A host file the egress sidecar writes a per-launch nonce into once its firewall is installed. The launch waits
/// for the nonce before it hands the sandbox out, so a sidecar whose rules failed never leaves a workload running
/// with open egress.
pub(crate) struct SidecarReadyMarker {
    path: PathBuf,
    nonce: String,
}

impl SidecarReadyMarker {
    /// Creates the empty marker under `dir`. It is world-writable because the sidecar's root keeps no
    /// `DAC_OVERRIDE`; the nonce, not the file's owner, is what proves the sidecar wrote it.
    pub(crate) async fn create(dir: &Path) -> Result<Self, DockerError> {
        let path = dir.join(format!("sandbox-egress-ready-{}", uuid::Uuid::new_v4()));
        let marker = Self {
            nonce: uuid::Uuid::new_v4().simple().to_string(),
            path,
        };
        tokio::fs::write(&marker.path, b"")
            .await
            .map_err(|source| marker.error(source))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            tokio::fs::set_permissions(&marker.path, std::fs::Permissions::from_mode(0o666))
                .await
                .map_err(|source| marker.error(source))?;
        }
        Ok(marker)
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    pub(crate) fn nonce(&self) -> &str {
        &self.nonce
    }

    pub(crate) async fn is_written(&self) -> Result<bool, DockerError> {
        let content = tokio::fs::read_to_string(&self.path)
            .await
            .map_err(|source| self.error(source))?;
        Ok(content.trim() == self.nonce)
    }

    /// Deletes the host file. The sidecar keeps its bind mount; nothing reads the marker again.
    pub(crate) async fn remove(self) {
        if let Err(error) = tokio::fs::remove_file(&self.path).await {
            tracing::warn!(path = %self.path.display(), %error, "failed to remove the egress sidecar readiness marker");
        }
    }

    fn error(&self, source: std::io::Error) -> DockerError {
        DockerError::EgressReadyMarker {
            path: self.path.clone(),
            source,
        }
    }
}

#[cfg(test)]
mod tests;
