use super::*;

impl DockerWorkerProvider {
    /// Installs `public_key` (a full `authorized_keys` line) into the
    /// container's `~/.ssh/authorized_keys` for `lease_id`, tracking
    /// `session_id -> public_key` so [`Self::ssh_detach_for_lease`] can strip
    /// precisely this line later without disturbing other concurrent SSH
    /// sessions on the same container.
    pub async fn ssh_attach_for_lease(
        &self,
        lease_id: &LeaseId,
        session_id: &str,
        public_key: &str,
    ) -> anyhow::Result<()> {
        let container_id = self.active_container_id(lease_id).await?;
        let escaped_key = public_key.replace('\'', "'\\''");
        let script = format!(
            "mkdir -p ~/.ssh && chmod 700 ~/.ssh && echo '{escaped_key}' >> ~/.ssh/authorized_keys && chmod 600 ~/.ssh/authorized_keys"
        );
        let out = Self::exec_collect(
            &self.docker,
            &container_id,
            vec!["sh".to_string(), "-c".to_string(), script],
            None,
            &std::collections::BTreeMap::new(),
            ExecStdin::None,
            None,
            None,
            None,
            OutputRetention::Buffer,
        )
        .await?;
        if out.exit_code != 0 {
            anyhow::bail!(
                "ssh attach failed (exit {}): {}",
                out.exit_code,
                String::from_utf8_lossy(&out.stderr)
            );
        }

        self.ssh_sessions
            .lock()
            .await
            .insert(session_id.to_string(), (lease_id.clone(), public_key.to_string()));
        Ok(())
    }

    /// Removes the `authorized_keys` line installed by
    /// [`Self::ssh_attach_for_lease`] for `session_id`, leaving other sessions'
    /// keys intact. A no-op (not an error) when the session is already gone.
    ///
    /// Filters via `grep -vF` fed the key over stdin rather than interpolating
    /// it into a shell command (as `sed`/`awk` matching would require), so
    /// arbitrary `authorized_keys` line content (e.g. embedded `/` or `#`)
    /// never needs shell-metacharacter escaping.
    pub async fn ssh_detach_for_lease(&self, lease_id: &LeaseId, session_id: &str) -> anyhow::Result<()> {
        let Some((_, public_key)) = self.ssh_sessions.lock().await.remove(session_id) else {
            return Ok(());
        };
        let container_id = self.active_container_id(lease_id).await?;
        let script = "f=~/.ssh/authorized_keys; [ -f \"$f\" ] || exit 0; \
            key=$(cat); tmp=\"$f.tmp.$$\"; grep -vF \"$key\" \"$f\" > \"$tmp\" || true; \
            mv \"$tmp\" \"$f\"";
        let out = Self::exec_collect(
            &self.docker,
            &container_id,
            vec!["sh".to_string(), "-c".to_string(), script.to_string()],
            None,
            &std::collections::BTreeMap::new(),
            ExecStdin::Once(public_key.into_bytes()),
            None,
            None,
            None,
            OutputRetention::Buffer,
        )
        .await?;
        if out.exit_code != 0 {
            anyhow::bail!(
                "ssh detach failed (exit {}): {}",
                out.exit_code,
                String::from_utf8_lossy(&out.stderr)
            );
        }
        Ok(())
    }

    pub(crate) async fn checkpoint_container(
        &self,
        container_id: &str,
        target: &SnapshotPushTarget,
    ) -> anyhow::Result<SnapshotReceipt> {
        let checkpoint_name = format!("trogon-chk-{}", crate::fresh_id::fresh_id());
        let checkpoint_dir = checkpoint_storage_dir(container_id, &checkpoint_name);
        let checkpoint_dir_str = checkpoint_dir.to_string_lossy().into_owned();

        self.inner
            .checkpoint_running(container_id, &checkpoint_name, &checkpoint_dir_str)
            .await
            .context("CRIU checkpoint create")?;

        let status = self
            .inner
            .engine()
            .inspect(container_id)
            .await
            .context("inspect container after checkpoint")?;
        if status != InspectStatus::Running {
            anyhow::bail!("expected container {container_id} to remain running after checkpoint, got {status:?}");
        }

        let raw_tar = tar_checkpoint_dir(&checkpoint_dir).context("tar checkpoint dir")?;
        let gz_tar = gzip_checkpoint(&raw_tar).context("gzip checkpoint tar")?;

        let registry = if target.registry.is_empty() {
            self.default_registry.as_str()
        } else {
            target.registry.as_str()
        };
        let (artifact_ref, bytes) =
            push_checkpoint_artifact(&self.oci_client, registry, &target.repository, gz_tar, &checkpoint_name).await?;

        if let Err(e) = tokio::fs::remove_dir_all(&checkpoint_dir).await {
            warn!(
                checkpoint_dir = %checkpoint_dir.display(),
                error = %e,
                "failed to remove local checkpoint dir after push"
            );
        }

        self.digest_cache.touch(artifact_ref.digest.clone());
        Ok(SnapshotReceipt {
            digest: artifact_ref.digest,
            bytes,
            artifact_type: ArtifactType::SandboxRootfsV1,
        })
    }
}
