use super::*;

/// Image ref a snapshot digest is restored to locally, matching the tag
/// `DockerWorkerProvider::restore_snapshot_image` imports under. Recomputing
/// it here (rather than only inside that method) lets the warm-pool reset
/// path label a reused container's [`WarmContainer::image_ref`] without an
/// extra OCI pull for a digest it already knows is resident.
pub(super) fn restored_image_tag(digest: &Digest) -> String {
    format!("trogon-sandbox-restore:restored-{}", digest.as_str().replace(':', "-"))
}

impl DockerWorkerProvider {
    pub(super) async fn restore_snapshot_image(&self, reference: &OciArtifactRef) -> anyhow::Result<String> {
        verify_if_configured(self.cosign_verifier.as_ref(), reference)
            .await
            .map_err(trogon_sandbox_cosign::into_pull_error)?;
        let pulled = pull_snapshot_layer(&self.oci_client, reference).await?;
        if layer_is_checkpoint(&pulled.annotations) {
            anyhow::bail!("checkpoint artifact requires SandboxImage::Snapshot.parent and live restore path");
        }

        if self.digest_cache.contains(&reference.digest) {
            info!(digest = %reference.digest, "snapshot digest cache hit");
            self.digest_cache.touch(reference.digest.clone());
        } else {
            let raw_tar = gunzip_tar(&pulled.data)?;
            let image_ref = self
                .inner
                .import_rootfs_image(&raw_tar, &restored_image_tag(&reference.digest))
                .await?;
            self.digest_cache.touch(reference.digest.clone());
            return Ok(image_ref);
        }

        Ok(restored_image_tag(&reference.digest))
    }

    pub(super) async fn try_restore_live_checkpoint(
        &self,
        reference: &OciArtifactRef,
        parent: Option<&OciImageRef>,
        req: &LeaseRequest,
        lease_id: &LeaseId,
        assignment: Option<&trogon_sandbox_contract::LeaseAttemptIdentity>,
    ) -> anyhow::Result<Option<ProviderLease>> {
        let pulled = pull_snapshot_layer(&self.oci_client, reference).await?;
        if !layer_is_checkpoint(&pulled.annotations) {
            return Ok(None);
        }

        let checkpoint_name = checkpoint_name_from_annotations(&pulled.annotations)
            .ok_or_else(|| anyhow::anyhow!("checkpoint artifact missing trogon.docker.checkpoint.name annotation"))?;
        let parent = parent.ok_or_else(|| {
            anyhow::anyhow!("live checkpoint restore requires SandboxImage::Snapshot.parent base image ref")
        })?;

        let extract_dir = std::env::temp_dir().join(format!(
            "sandbox-checkpoint-restore-{}-{}",
            lease_id.as_str(),
            crate::fresh_id::fresh_id()
        ));
        untar_checkpoint(&pulled.data, &extract_dir)?;

        let cpu = lease_cpus(&req.spec);
        let runtime_mount = self.prepare_runtime_mount(&parent.allowlist_string()).await?;
        let run_spec = ContainerRunSpec {
            name: format!("restored-chk-{}", crate::fresh_id::fresh_id()),
            image: parent.allowlist_string(),
            memory_mib: req.spec.memory_mib().get(),
            cpu,
            env: vec![],
            binds: runtime_mount
                .as_ref()
                .map(|mount| vec![mount.binding()])
                .unwrap_or_default(),
            network_mode: "none".to_string(),
            entrypoint_override: None,
            cmd_override: None,
            working_dir: None,
            user: self.inner.default_user().map(String::from),
            cap_add: self.inner.hardening().capability_sets(&[]).0,
            cap_drop: self.inner.hardening().capability_sets(&[]).1,
            exposed_ports: vec![],
            hardening: self.inner.hardening().clone(),
            disk_mib: req.spec.disk_mib(),
            labels: self.inner.owner_labels("restored", Some(lease_id.as_str())),
        };

        let container_id = self
            .inner
            .restore_from_checkpoint(run_spec, &checkpoint_name, extract_dir.to_string_lossy().as_ref())
            .await
            .context("restore docker checkpoint")?;

        self.digest_cache.touch(reference.digest.clone());
        let registration_id = crate::fresh_id::fresh_id();
        self.active.lock().await.insert(
            lease_id.as_str().to_owned(),
            ActiveDockerLease {
                runtime_mount: runtime_mount.clone(),
                registration_id: registration_id.clone(),
                dispatch_target: None,
                dispatch_target_bound: false,
                container_id: container_id.clone(),
                snapshot_digest: Some(reference.digest.clone()),
                snapshot_reference: Some(reference.clone()),
                memory_mib: req.spec.memory_mib().get(),
                cpu,
                policy_file_path: None,
                sidecar_id: None,
                exposed_port_endpoints: HashMap::new(),
                request: Box::new(req.clone()),
                // Restores the checkpointed process as PID 1, not `sleep
                // infinity`; the reset script's "kill everything but PID 1"
                // would leave that workload running for the next tenant.
                warm_pool_eligible: false,
            },
        );

        self.remember_created_native(
            lease_id,
            &registration_id,
            &container_id,
            self.inner.owner_labels("restored", Some(lease_id.as_str())),
            req.spec.memory_mib().get(),
            cpu,
            Some(reference.digest.clone()),
        )
        .await?;
        let receipt = self
            .retain_allocated_assignment(lease_id, &container_id, assignment)
            .await?;
        self.pin_created_native(lease_id, &registration_id, &container_id)
            .await?;

        if let (Some(runtime), Some(mount)) = (&self.command_runtime, &runtime_mount)
            && let Err(error) = runtime.verify_mount(&self.docker, &container_id, mount).await
        {
            self.cancel_locked(lease_id)
                .await
                .context("retire a restored container with unverified runtime provenance")?;
            return Err(error);
        }

        self.rollback_state.remember(lease_id, req).await;
        self.record_start_baseline(lease_id, &container_id).await;
        Ok(Some(receipt))
    }
}
