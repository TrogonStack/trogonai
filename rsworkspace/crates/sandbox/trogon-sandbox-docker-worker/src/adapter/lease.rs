use super::*;

impl DockerWorkerProvider {
    pub(crate) fn cancel_locked<'a>(
        &'a self,
        lease_id: &'a LeaseId,
    ) -> futures::future::BoxFuture<'a, anyhow::Result<()>> {
        Box::pin(async move {
            let Some(active) = self.active.lock().await.get(lease_id.as_str()).cloned() else {
                return Ok(());
            };
            if self.container_is_paused(&active.container_id).await.unwrap_or(false) {
                self.docker.unpause_container(&active.container_id).await?;
            }
            let result = if active.warm_pool_eligible {
                if let (Some(digest), Some(reference)) =
                    (active.snapshot_digest.clone(), active.snapshot_reference.clone())
                {
                    self.begin_native_retirement(lease_id, &active).await?;
                    self.forget_executions(lease_id).await;
                    self.tracking.forget(lease_id.as_str());
                    self.inotify_missing.forget(lease_id.as_str());
                    self.watch_feeds.forget(lease_id.as_str());
                    self.rollback_state.forget(lease_id).await;
                    self.recycle_or_refill_warm_pool(
                        lease_id,
                        &active,
                        digest.clone(),
                        reference,
                        restored_image_tag(&digest),
                    )
                    .await
                } else {
                    self.teardown_container(lease_id, &active.container_id, active.sidecar_id.as_deref())
                        .await
                }
            } else {
                self.teardown_container(lease_id, &active.container_id, active.sidecar_id.as_deref())
                    .await
            };
            if result.is_ok() {
                self.forget_executions(lease_id).await;
                self.rollback_state.forget(lease_id).await;
                self.active.lock().await.remove(lease_id.as_str());
                self.authority_paused
                    .lock()
                    .await
                    .retain(|owned| owned.lease_id() != lease_id || !Self::authority_matches(&active, owned));
            }
            result
        })
    }

    pub(super) async fn lease_locked(&self, req: &LeaseRequest, lease_id: &LeaseId) -> anyhow::Result<ProviderLease> {
        self.lease_locked_with_assignment(req, lease_id, None).await
    }

    pub(super) async fn lease_locked_with_assignment(
        &self,
        req: &LeaseRequest,
        lease_id: &LeaseId,
        assignment: Option<&trogon_sandbox_contract::LeaseAttemptIdentity>,
    ) -> anyhow::Result<ProviderLease> {
        trogon_sandbox_protocol::reject_unsupported_privileged(&req.spec, &crate::profile::PROFILE)?;
        // Refuse bad native options before any image pull or container work.
        let native_hardening = hardening_with_native(self.inner.hardening(), req.native.as_ref())?;
        let browser = crate::browser::admit_browser(req, self.browser_support.as_ref(), self.inner.hardening())?;

        if browser.is_none() {
            login_for_pull_secret(
                &self.nats_url,
                &req.tenant,
                req.spec.image(),
                req.ttl_seconds,
                &trogon_std::env::SystemEnv,
            )
            .await?;
        }

        if let SandboxImage::Snapshot { reference, parent } = req.spec.image()
            && let Some(lease) = self
                .try_restore_live_checkpoint(reference, parent.as_ref(), req, lease_id, assignment)
                .await?
        {
            return Ok(lease);
        }

        // Warm containers are shaped at refill time, so a lease that asks for
        // its own size cannot be handed one.
        // The same holds for hardening overrides: a pooled container was
        // created with the worker's own hardening.
        let overridden = !req.spec.platform().container.is_default();
        let sized = req.spec.vcpu_millis().is_some()
            || req.spec.disk_mib().is_some()
            || overridden
            || native_hardening.is_some();
        if let SandboxImage::Snapshot { reference, .. } = req.spec.image()
            && !sized
            && Self::workspace_allows_warm_pool(req.spec.workspace())
            && Self::network_policy_allows_warm_pool(req.spec.network_policy())
            && let Some(warm) = self.warm_pool.lock().await.take(&reference.digest)
        {
            let cpu = lease_cpus(&req.spec);
            let registration_id = crate::fresh_id::fresh_id();
            self.claim_warm_native(&warm, lease_id, &registration_id, req.spec.memory_mib().get(), cpu)
                .await?;
            info!(
                digest = %reference.digest,
                container_id = %warm.container_id,
                "warm pool hit"
            );
            self.warm_grab_count.fetch_add(1, Ordering::Relaxed);
            self.active.lock().await.insert(
                lease_id.as_str().to_owned(),
                ActiveDockerLease {
                    runtime_mount: None,
                    registration_id,
                    dispatch_target: None,
                    dispatch_target_bound: false,
                    container_id: warm.container_id.clone(),
                    snapshot_digest: Some(reference.digest.clone()),
                    snapshot_reference: Some(reference.clone()),
                    memory_mib: req.spec.memory_mib().get(),
                    cpu,
                    policy_file_path: None,
                    sidecar_id: None,
                    exposed_port_endpoints: HashMap::new(),
                    warm_pool_eligible: true,
                    request: Box::new(req.clone()),
                },
            );
            let receipt = self
                .retain_allocated_assignment(lease_id, &warm.container_id, assignment)
                .await?;
            let held = self
                .active
                .lock()
                .await
                .get(lease_id.as_str())
                .cloned()
                .context("claimed warm registration missing")?;
            if let Err(error) = self.verify_held_native_birth(lease_id, &held).await {
                if let Some(held) = self.active.lock().await.get_mut(lease_id.as_str()) {
                    held.warm_pool_eligible = false;
                }
                return Err(error);
            }
            if let Some(runtime) = &self.command_runtime {
                match runtime.existing_mount(&self.docker, &warm.container_id).await {
                    Ok(mount) => {
                        self.active
                            .lock()
                            .await
                            .get_mut(lease_id.as_str())
                            .context("claimed warm registration disappeared")?
                            .runtime_mount = Some(mount);
                    }
                    Err(error) => {
                        self.active
                            .lock()
                            .await
                            .get_mut(lease_id.as_str())
                            .context("claimed warm registration disappeared")?
                            .warm_pool_eligible = false;
                        return Err(error);
                    }
                }
            }
            self.record_start_baseline(lease_id, &warm.container_id).await;
            return Ok(receipt);
        }

        self.cold_start_count.fetch_add(1, Ordering::Relaxed);

        let (prepared, _scratch) = self.prepare_lease_request(req, lease_id).await?;

        let snapshot_meta = match req.spec.image() {
            SandboxImage::Snapshot { reference, .. } => Some((reference.digest.clone(), reference.clone())),
            _ => None,
        };

        let cpu = lease_cpus(&req.spec);

        let mut spec = translate_lease_request_to_sandbox_spec(&prepared, lease_id)?;
        if let Some(browser) = browser.as_ref() {
            spec.image = trogon_sandbox_providers::ImageRef::new(browser.image_id.clone());
            spec.user = Some(browser.user.clone());
            spec.env.extend(browser.env.clone());
            if spec.working_dir.is_none() {
                spec.working_dir = Some("/tmp".into());
            }
        }
        let runtime_mount = self.prepare_runtime_mount(spec.image.as_str()).await?;
        let (fuse_handles, fuse_binds) = if req.spec.mounts().is_empty() {
            (vec![], vec![])
        } else {
            let (handles, mounts_with_dirs) = self.mount_fuse_volumes(req, lease_id).await?;
            let binds = build_fuse_bind_strings(&mounts_with_dirs);
            (handles, binds)
        };
        spec.extra_binds = fuse_binds;
        if let Some(mount) = &runtime_mount {
            spec.extra_binds.push(mount.binding());
        }

        let lifecycle = Arc::new(NatsLifecycleEmitter::new(self.js.clone(), lifecycle_subject(lease_id)));
        let mut hardening = crate::hardening_overrides::hardening_with_overrides(
            self.inner.hardening(),
            &req.spec.platform().container,
        );
        if let Some(native) = hardening_with_native(&hardening, req.native.as_ref())? {
            hardening = native;
        }
        if let Some(browser) = browser {
            hardening = browser.hardening;
        }
        let inner = self
            .inner
            .with_lifecycle_emitter(Some(Arc::clone(&lifecycle) as _))
            .with_hardening(hardening);
        let requested_boot_mode = spec.boot_mode;
        let launched = crate::launch_guard::launch_alive(requested_boot_mode, |boot_mode| {
            let mut spec = spec.clone();
            spec.boot_mode = boot_mode;
            inner.launch(spec)
        })
        .await;
        let handle = match launched {
            Ok(handle) => handle,
            Err(e) => {
                for h in fuse_handles {
                    let _ = h.unmount().await;
                }
                return Err(e.into());
            }
        };
        let sandbox_id = handle.id().to_string();
        let policy_file_path = handle.policy_file_path().map(std::path::Path::to_path_buf);
        let sidecar_id = handle.sidecar_id().map(str::to_string);
        let exposed_port_endpoints = handle.port_endpoints();
        // Failure events name the lease, as the lifecycle consumers key by lease id.
        self.watch_for_exit(
            handle,
            lease_id,
            sandbox_id.clone(),
            SandboxId::new(lease_id.as_str())?,
            lifecycle,
        );

        if !req.spec.mounts().is_empty() {
            self.fuse_handles
                .lock()
                .await
                .insert(lease_id.as_str().to_owned(), fuse_handles);
        }

        let warm_pool_eligible = snapshot_meta.is_some()
            && !sized
            && Self::workspace_allows_warm_pool(req.spec.workspace())
            && Self::network_policy_allows_warm_pool(req.spec.network_policy());
        let registration_id = crate::fresh_id::fresh_id();
        let snapshot_digest = snapshot_meta.as_ref().map(|(digest, _)| digest.clone());
        self.active.lock().await.insert(
            lease_id.as_str().to_owned(),
            ActiveDockerLease {
                runtime_mount: runtime_mount.clone(),
                registration_id: registration_id.clone(),
                dispatch_target: None,
                dispatch_target_bound: false,
                container_id: sandbox_id.clone(),
                snapshot_digest: snapshot_digest.clone(),
                snapshot_reference: snapshot_meta.map(|(_, reference)| reference),
                memory_mib: req.spec.memory_mib().get(),
                cpu,
                policy_file_path,
                sidecar_id,
                exposed_port_endpoints,
                warm_pool_eligible,
                request: Box::new(req.clone()),
            },
        );
        self.remember_created_native(
            lease_id,
            &registration_id,
            &sandbox_id,
            self.inner.owner_labels("workload", Some(lease_id.as_str())),
            req.spec.memory_mib().get(),
            cpu,
            snapshot_digest,
        )
        .await?;
        let receipt = self
            .retain_allocated_assignment(lease_id, &sandbox_id, assignment)
            .await?;
        if let Err(error) = self.pin_created_native(lease_id, &registration_id, &sandbox_id).await {
            if let Some(held) = self.active.lock().await.get_mut(lease_id.as_str()) {
                held.warm_pool_eligible = false;
            }
            return Err(error);
        }
        if let (Some(runtime), Some(mount)) = (&self.command_runtime, &runtime_mount)
            && let Err(error) = runtime.verify_mount(&self.docker, &sandbox_id, mount).await
        {
            if let Some(held) = self.active.lock().await.get_mut(lease_id.as_str()) {
                held.warm_pool_eligible = false;
            }
            self.cancel_locked(lease_id)
                .await
                .context("retire a container with unverified runtime provenance")?;
            return Err(error);
        }
        self.rollback_state.remember(lease_id, req).await;
        self.record_start_baseline(lease_id, &sandbox_id).await;
        Ok(receipt)
    }

    pub(super) async fn prepare_runtime_mount(
        &self,
        image: &str,
    ) -> anyhow::Result<Option<command_runtime::RuntimeMount>> {
        match &self.command_runtime {
            Some(runtime) => runtime.mount(&self.docker, image).await.map(Some),
            None => Ok(None),
        }
    }

    pub(super) async fn mount_fuse_volumes(
        &self,
        req: &LeaseRequest,
        lease_id: &LeaseId,
    ) -> anyhow::Result<(Vec<FuseMountHandle>, Vec<(RemoteMount, PathBuf)>)> {
        let fuse_base = std::env::temp_dir().join(format!("sandbox-fuse-{}", lease_id));

        let mut handles: Vec<FuseMountHandle> = Vec::with_capacity(req.spec.mounts().len());
        let mut mounts_with_dirs: Vec<(RemoteMount, PathBuf)> = Vec::with_capacity(req.spec.mounts().len());

        for (idx, mount) in req.spec.mounts().iter().enumerate() {
            let host_dir = fuse_base.join(idx.to_string());
            match self
                .fuse_manager
                .mount(
                    &mount.source,
                    &host_dir,
                    HostMountAccess::from(mount.access.is_read_only()),
                )
                .await
            {
                Ok(handle) => {
                    mounts_with_dirs.push((mount.clone(), host_dir));
                    handles.push(handle);
                }
                Err(e) => {
                    for h in handles {
                        let _ = h.unmount().await;
                    }
                    return Err(anyhow::anyhow!("failed to mount {}: {e}", mount.mount_path));
                }
            }
        }

        Ok((handles, mounts_with_dirs))
    }

    pub(super) async fn prepare_lease_request(
        &self,
        req: &LeaseRequest,
        lease_id: &LeaseId,
    ) -> anyhow::Result<(LeaseRequest, Option<PathBuf>)> {
        let mut req = req.clone();
        let mut scratch_dir: Option<PathBuf> = None;

        let workspace = req.spec.workspace().clone();
        if let WorkspaceSource::S3Volume {
            bucket,
            prefix,
            mode,
            snapshot: _,
        } = &workspace
        {
            let region = match self.volume_region.clone() {
                Some(region) => region,
                None => VolumeRegion::new(DEFAULT_VOLUME_REGION)?,
            };
            let dir = std::env::temp_dir().join(format!(
                "sandbox-s3vol-{}-{}",
                lease_id.as_str(),
                crate::fresh_id::fresh_id()
            ));
            tokio::fs::create_dir_all(&dir)
                .await
                .context("create s3 volume scratch dir")?;

            let config = VolumeClientConfig {
                backend: VolumeBackend::S3 {
                    bucket: S3BucketName::new(bucket.as_str())?,
                    region,
                    endpoint: self.volume_endpoint.clone(),
                },
                kms_key_id: None,
            };
            let client = VolumeClient::with_mode(config, *mode).map_err(|e| anyhow::anyhow!("volume client: {e}"))?;

            client
                .pull_to_dir(&VolumePrefix::new(prefix.as_str())?, &dir)
                .await
                .map_err(|e| anyhow::anyhow!("s3 volume pull: {e}"))?;

            scratch_dir = Some(dir.clone());
            req.spec = spec_edit::with_workspace(
                req.spec.clone(),
                WorkspaceSource::HostMount {
                    path: dir.to_string_lossy().into_owned(),
                    access: HostMountAccess::from(matches!(mode, VolumeMode::ReadOnly)),
                },
            )?;
        }

        if let SandboxImage::Snapshot { reference, .. } = req.spec.image() {
            let image_ref = self
                .restore_snapshot_image(reference)
                .await
                .context("restore snapshot image")?;
            let image = SandboxImage::Registry {
                reference: OciImageRef::parse_unqualified(&image_ref),
                pull_secret: None,
            };
            req.spec = spec_edit::with_image(req.spec.clone(), image)?;
        }

        Ok((req, scratch_dir))
    }
}
