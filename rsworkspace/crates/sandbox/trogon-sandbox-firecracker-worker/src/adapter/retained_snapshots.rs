use super::*;

impl FirecrackerProvider {
    pub fn with_oci_client(mut self, oci: Arc<trogon_sandbox_oci::OciClient>) -> Self {
        self.images = self
            .config
            .oci_rootfs
            .clone()
            .map(|config| crate::rootfs::ImageCache::new(config, Arc::clone(&oci)));
        self.oci = oci;
        self
    }

    pub async fn sweep_warm_pool(&self) {
        self.warm_pool.lock().await.sweep();
        if let Err(error) = self.sweep_retained_at(std::time::SystemTime::now()).await {
            warn!(%error, "released snapshot cleanup failed");
        }
    }

    pub(super) async fn depot(&self) -> anyhow::Result<&Arc<crate::retained::SnapshotDepot>> {
        self.retained
            .get_or_try_init(|| async {
                self.recover_stale().await?;
                let worker = self.dispatch_ledger().await?.worker_id().clone();
                let depot = Arc::new(crate::retained::SnapshotDepot::open(
                    &self.config.state_dir.as_path().join(crate::retained::DIRECTORY),
                    &worker,
                    self.config.released_snapshots.clone(),
                )?);
                depot.recover().await?;
                Ok(depot)
            })
            .await
    }

    pub(crate) async fn sweep_retained_at(&self, now: std::time::SystemTime) -> anyhow::Result<()> {
        if self.retained.get().is_none()
            && !self
                .config
                .state_dir
                .as_path()
                .join(crate::retained::DIRECTORY)
                .exists()
        {
            return Ok(());
        }
        let depot = self.depot().await?;
        for lease in depot.expired(now)? {
            let Some(_gate) = self.lease_gates.try_lock(&lease) else {
                continue;
            };
            if self.active.lock().await.contains_key(lease.as_str()) {
                continue;
            }
            depot.mark_purged(&lease).await?;
            depot.delete_purged(&lease)?;
        }
        Ok(())
    }

    pub async fn clear_warm_pool(&self) {
        self.warm_pool.lock().await.clear();
    }

    pub async fn snapshot_inner(
        &self,
        lease_id: &LeaseId,
        target: &trogon_sandbox_contract::OciArtifactRef,
    ) -> anyhow::Result<trogon_sandbox_contract::SnapshotReceipt> {
        self.snapshot_to(
            lease_id,
            &trogon_sandbox_contract::SnapshotPushTarget {
                registry: target.registry.clone(),
                repository: target.repository.clone(),
                artifact_type: target.artifact_type,
            },
        )
        .await
    }

    pub async fn snapshot_to(
        &self,
        lease_id: &LeaseId,
        target: &trogon_sandbox_contract::SnapshotPushTarget,
    ) -> anyhow::Result<trogon_sandbox_contract::SnapshotReceipt> {
        if self.snapshots.is_some()
            && self
                .active
                .lock()
                .await
                .get(lease_id.as_str())
                .is_some_and(|vm| vm.snapshot_policy.is_none())
        {
            return self.checkpoint_inner(lease_id).await.map_err(Into::into);
        }
        anyhow::ensure!(
            target.artifact_type == trogon_sandbox_contract::ArtifactType::FirecrackerSnapshotV1,
            "snapshot target must use FirecrackerSnapshotV1"
        );
        let _gate = self.lease_gates.lock(lease_id).await;
        if self.retained.get().is_some()
            || self
                .config
                .state_dir
                .as_path()
                .join(crate::retained::DIRECTORY)
                .exists()
        {
            let depot = self.depot().await?;
            if let Some(entry) = depot.get(lease_id)? {
                if entry.expires_at <= std::time::SystemTime::now()
                    && !self.active.lock().await.contains_key(lease_id.as_str())
                {
                    depot.mark_purged(lease_id).await?;
                    return Err(crate::retained::ReleaseExportError::Purged.into());
                }
                depot.check_target(&entry, target)?;
                depot.recover_lease(lease_id).await?;
                if let Some(receipt) = depot.receipt(lease_id, target)? {
                    depot.cleanup_files(lease_id).await?;
                    return Ok(receipt);
                }
                self.retain_armed(lease_id, depot).await?;
                depot.check_target(
                    &depot
                        .get(lease_id)?
                        .ok_or(crate::retained::ReleaseExportError::Missing)?,
                    target,
                )?;
                depot.claim_target(lease_id, target)?;
                let receipt = tokio::time::timeout(
                    std::time::Duration::from_secs(180),
                    crate::snapshot::push_directory(&self.oci, target, &depot.directory(lease_id)),
                )
                .await??;
                depot.record_receipt(lease_id, target, &receipt)?;
                depot.cleanup_files(lease_id).await?;
                return Ok(receipt);
            }
        }
        self.capture_active(lease_id).await?.push(&self.oci, target).await
    }

    pub async fn checkpoint_to(
        &self,
        lease_id: &LeaseId,
        target: &trogon_sandbox_contract::SnapshotPushTarget,
    ) -> anyhow::Result<trogon_sandbox_contract::SnapshotReceipt> {
        if self.snapshots.is_some() {
            return self.checkpoint_inner(lease_id).await.map_err(Into::into);
        }
        anyhow::ensure!(
            target.artifact_type == trogon_sandbox_contract::ArtifactType::FirecrackerSnapshotV1,
            "snapshot target must use FirecrackerSnapshotV1"
        );
        let _gate = self.lease_gates.lock(lease_id).await;
        self.capture_active(lease_id).await?.push(&self.oci, target).await
    }

    pub(super) async fn capture_active(&self, lease_id: &LeaseId) -> anyhow::Result<crate::snapshot::LocalSnapshot> {
        let snapshot = {
            let active = self.active.lock().await;
            let vm = active
                .get(lease_id.as_str())
                .ok_or(crate::retained::ReleaseExportError::Missing)?;
            let mut metadata = crate::snapshot::SnapshotMetadata::new(vm.memory, self.config.default_vcpus)
                .with_env_digest(self.config.environment.own.clone());
            metadata.image_defaults = vm.image_defaults.clone();
            metadata.network_policy = vm.network.as_ref().map(|network| network.policy().clone());
            crate::snapshot::LocalSnapshot::capture(&vm.dir, metadata).await?
        };
        Ok(snapshot)
    }

    pub(super) async fn retain_armed(
        &self,
        lease: &LeaseId,
        depot: &crate::retained::SnapshotDepot,
    ) -> anyhow::Result<()> {
        depot.recover_lease(lease).await?;
        if !depot
            .get(lease)?
            .is_some_and(|entry| matches!(entry.state, crate::retained::RetainedState::Armed { .. }))
        {
            return Ok(());
        }
        let alive = self
            .active
            .lock()
            .await
            .get_mut(lease.as_str())
            .is_some_and(|vm| !vm.process.has_exited());
        if !alive {
            depot.fail_if_armed(lease, "source microVM exited before release snapshot capture")?;
            return Err(crate::retained::ReleaseExportError::Failed(
                "source microVM exited before release snapshot capture".into(),
            )
            .into());
        }
        let snapshot = self.capture_active(lease).await?;
        depot.retain(lease, snapshot).await
    }
}
