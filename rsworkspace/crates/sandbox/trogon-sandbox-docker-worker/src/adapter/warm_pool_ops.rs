use super::*;

impl DockerWorkerProvider {
    pub async fn sweep_warm_pool(&self) {
        let max_idle = {
            let pool = self.warm_pool.lock().await;
            Duration::from_secs(pool.config().max_idle_seconds)
        };
        let to_kill = {
            let mut pool = self.warm_pool.lock().await;
            pool.sweep_idle(max_idle)
        };
        for container_id in to_kill {
            if let Err(e) = self.dispose_pooled_native(&container_id).await {
                warn!(container_id = %container_id, error = %e, "failed to kill idle warm container");
            }
        }
    }

    pub(super) async fn kill_evicted(&self, container_ids: Vec<String>) {
        for container_id in container_ids {
            if let Err(e) = self.dispose_pooled_native(&container_id).await {
                warn!(container_id = %container_id, error = %e, "failed to kill evicted warm container");
            }
        }
    }

    pub(super) async fn start_warm_container(
        &self,
        image_ref: &str,
        digest: Digest,
        memory_mib: u32,
        cpu: f32,
    ) -> anyhow::Result<WarmContainer> {
        let container_name = format!("warm-{}", crate::fresh_id::fresh_id());
        let runtime_mount = self.prepare_runtime_mount(image_ref).await?;
        let run_spec = ContainerRunSpec {
            name: container_name,
            image: image_ref.to_string(),
            memory_mib,
            cpu,
            env: vec![],
            binds: runtime_mount
                .as_ref()
                .map(|mount| vec![mount.binding()])
                .unwrap_or_default(),
            network_mode: "none".to_string(),
            // `BootMode::Bare`, like a cold snapshot lease: an imported rootfs
            // has no command of its own, and the reset keeps PID 1 alive.
            entrypoint_override: Some(vec![]),
            cmd_override: Some(vec!["sleep".to_string(), "infinity".to_string()]),
            working_dir: None,
            user: self.inner.default_user().map(String::from),
            cap_add: self.inner.hardening().capability_sets(&[]).0,
            cap_drop: self.inner.hardening().capability_sets(&[]).1,
            exposed_ports: vec![],
            hardening: self.inner.hardening().clone(),
            disk_mib: None,
            labels: self.inner.owner_labels("warm", None),
        };
        let container_id = self
            .inner
            .engine()
            .create_and_start(run_spec)
            .await
            .context("warm container start failed")?
            .container_id;
        let registration = crate::fresh_id::fresh_id();
        let warm_lease = LeaseId::new(format!("warm-native-{registration}"));
        self.remember_created_native(
            &warm_lease,
            &registration,
            &container_id,
            self.inner.owner_labels("warm", None),
            memory_mib,
            cpu,
            Some(digest.clone()),
        )
        .await?;
        if let Err(error) = self.pin_created_native(&warm_lease, &registration, &container_id).await {
            self.inner.engine().remove(&container_id).await?;
            self.confirm_container_absence(&container_id).await?;
            self.native_custody.lock().await.remove(&container_id);
            return Err(error);
        }
        if let (Some(runtime), Some(mount)) = (&self.command_runtime, &runtime_mount)
            && let Err(error) = runtime.verify_mount(&self.docker, &container_id, mount).await
        {
            self.inner.engine().remove(&container_id).await?;
            self.confirm_container_absence(&container_id).await?;
            self.native_custody.lock().await.remove(&container_id);
            return Err(error);
        }
        self.publish_created_warm_native(&warm_lease, &registration, &container_id, digest, image_ref.into())
            .await
    }

    /// Wipes the previous tenant's workspace and kills any processes it left
    /// running, via `docker exec` on the still-running `container_id`, so the
    /// container can be handed to a different tenant without a full
    /// teardown/recreate cycle.
    ///
    /// Every warm-pool-eligible lease uses `WorkspaceSource::Empty` (see
    /// `workspace_allows_warm_pool`), which `translate_lease_request_to_sandbox_spec`
    /// maps to no `clone`/`host_mount`, so `LocalDockerProvider::launch` never
    /// bind-mounts `WORKSPACE_MOUNT_PATH` for these containers -- there is no
    /// dedicated workspace subtree to isolate. The reset therefore targets
    /// both the conventional workspace path (defensively, in case a tenant
    /// created it) and `/tmp`, the other universally-writable location a
    /// tenant would use in its absence, in addition to killing leftover
    /// processes. This crate has no PID-tracking or env/credential injection
    /// mechanism for running containers (env is only ever set at container
    /// creation, see `start_warm_container`), so there is nothing else to
    /// clear.
    ///
    /// Returns `Err` if any step fails or exits non-zero -- callers must
    /// treat that as "container may still hold tenant A's state" and fall
    /// back to destroy-and-recreate rather than returning it to the pool.
    pub(super) async fn reset_container_in_place(&self, container_id: &str) -> anyhow::Result<()> {
        // Warm-pool containers are launched with `BootMode::Bare`, whose PID 1
        // is `sleep infinity` (see `LocalDockerProvider::launch`); it must
        // survive the reset so the container stays up for the next lease.
        // Everything else -- any foreground/background process the previous
        // tenant started -- is fair game. `$$` is this exec'd shell's own
        // pid, excluded so it can finish running the rest of the script.
        let reset_script = format!(
            "for p in /proc/[0-9]*; do \
               p=${{p#/proc/}}; \
               if [ \"$p\" != 1 ] && [ \"$p\" != \"$$\" ]; then \
                 kill -9 \"$p\" 2>/dev/null; \
               fi; \
             done; \
             rm -rf {ws}/* {ws}/.[!.]* /tmp/* /tmp/.[!.]* 2>/dev/null; \
             mkdir -p {ws}",
            ws = WORKSPACE_MOUNT_PATH,
        );

        let outcome = Self::exec_collect(
            &self.docker,
            container_id,
            vec!["sh".to_string(), "-c".to_string(), reset_script],
            None,
            &std::collections::BTreeMap::new(),
            ExecStdin::None,
            None,
            None,
            None,
            OutputRetention::Buffer,
        )
        .await
        .context("warm pool reset exec failed")?;

        if outcome.exit_code != 0 {
            anyhow::bail!(
                "warm pool reset exited {} for container {container_id}: {}",
                outcome.exit_code,
                String::from_utf8_lossy(&outcome.stderr)
            );
        }

        Ok(())
    }

    /// Attempts to return `container_id` to the warm pool in place via
    /// [`Self::reset_container_in_place`]. On any reset failure this is a
    /// security boundary, not a best-effort cleanup: it falls back to the
    /// existing teardown-and-refill path rather than risk leaking a possibly
    /// dirty container to a different tenant.
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn recycle_or_refill_warm_pool(
        &self,
        lease_id: &LeaseId,
        active: &ActiveDockerLease,
        digest: Digest,
        reference: OciArtifactRef,
        image_ref: String,
    ) -> anyhow::Result<()> {
        self.verify_retiring_native_birth(lease_id, active).await?;
        let container_id = &active.container_id;
        match self.reset_container_in_place(container_id).await {
            Ok(()) => {
                info!(
                    container_id = %container_id,
                    digest = %digest,
                    "warm pool reset succeeded, returning container in place"
                );
                let claim = self
                    .publish_warm_native(lease_id, active, digest.clone(), image_ref)
                    .await?;
                let evicted = {
                    let mut pool = self.warm_pool.lock().await;
                    pool.insert(claim)
                };
                self.kill_evicted(evicted).await;
                // The returned container covers one slot; a cold first lease
                // leaves the rest of the digest's pool empty.
                self.refill_warm_pool(digest, reference, active.memory_mib, active.cpu)
                    .await;
                Ok(())
            }
            Err(e) => {
                warn!(
                    container_id = %container_id,
                    error = %e,
                    "warm pool reset failed, falling back to destroy-and-recreate"
                );
                self.teardown_container(lease_id, container_id, None).await?;
                self.refill_warm_pool(digest, reference, active.memory_mib, active.cpu)
                    .await;
                Ok(())
            }
        }
    }

    pub(super) async fn refill_warm_pool(&self, digest: Digest, reference: OciArtifactRef, memory_mib: u32, cpu: f32) {
        let enabled = {
            let pool = self.warm_pool.lock().await;
            pool.config().enabled
        };
        if !enabled {
            return;
        }

        let image_ref = match self.restore_snapshot_image(&reference).await {
            Ok(image_ref) => image_ref,
            Err(e) => {
                warn!(digest = %digest, error = %e, "warm pool refill skipped: restore failed");
                return;
            }
        };

        loop {
            let deficit = {
                let pool = self.warm_pool.lock().await;
                pool.config()
                    .max_pool_size_per_digest
                    .saturating_sub(pool.count_for_digest(&digest))
            };
            if deficit == 0 {
                break;
            }

            let at_total_cap = {
                let pool = self.warm_pool.lock().await;
                pool.total_count() >= pool.config().max_total_warm
            };
            if at_total_cap {
                break;
            }

            match self
                .start_warm_container(&image_ref, digest.clone(), memory_mib, cpu)
                .await
            {
                Ok(warm) => {
                    let evicted = {
                        let mut pool = self.warm_pool.lock().await;
                        pool.insert(warm)
                    };
                    self.kill_evicted(evicted).await;
                }
                Err(e) => {
                    warn!(digest = %digest, error = %e, "warm pool refill stopped");
                    break;
                }
            }
        }
    }

    pub(super) fn workspace_allows_warm_pool(workspace: &WorkspaceSource) -> bool {
        matches!(workspace, WorkspaceSource::Empty(_))
    }

    /// Warm containers are started with `network_mode: none` and no egress
    /// sidecar, which is exactly what a lease with no policy or `DenyAll`
    /// gets cold. A lease that names any other policy needs a bridge network
    /// (and, for `UserDefined`, a sidecar), so it cannot be handed a pooled
    /// container. The same holds in reverse: a container cold-started with
    /// that network must never be recycled into the pool, or the next
    /// `DenyAll` lease would inherit its open network.
    pub(super) fn network_policy_allows_warm_pool(policy: Option<&NetworkPolicy>) -> bool {
        matches!(policy, None | Some(NetworkPolicy::DenyAll))
    }

    pub(super) async fn teardown_container(
        &self,
        lease_id: &LeaseId,
        container_id: &str,
        sidecar_id: Option<&str>,
    ) -> anyhow::Result<()> {
        let lifecycle = Arc::new(NatsLifecycleEmitter::new(self.js.clone(), lifecycle_subject(lease_id)));
        self.tracking.forget(lease_id.as_str());
        self.inotify_missing.forget(lease_id.as_str());
        self.watch_feeds.forget(lease_id.as_str());
        if let Some(sidecar_id) = sidecar_id
            && let Err(e) = self.inner.engine().kill(sidecar_id).await
        {
            warn!(lease_id = %lease_id, error = %e, "egress sidecar teardown failed");
        }
        self.inner.tear_down(container_id, Some(lifecycle.as_ref())).await?;
        self.confirm_container_absence(container_id).await?;
        self.native_custody.lock().await.remove(container_id);

        let handles = self
            .fuse_handles
            .lock()
            .await
            .remove(lease_id.as_str())
            .unwrap_or_default();
        for handle in handles {
            if let Err(e) = handle.unmount().await {
                warn!(lease_id = %lease_id, error = %e, "fuse unmount failed on teardown");
            }
        }

        Ok(())
    }

    pub(super) async fn confirm_container_absence(&self, container_id: &str) -> anyhow::Result<()> {
        let confirmation = async {
            loop {
                match self.docker.inspect_container(container_id, None).await {
                    Err(bollard::errors::Error::DockerResponseServerError { status_code: 404, .. }) => return Ok(()),
                    Ok(container) => {
                        anyhow::ensure!(
                            container.id.as_deref() == Some(container_id),
                            "container cleanup inspection returned another identity"
                        );
                    }
                    Err(error) => {
                        return Err(anyhow::Error::new(error).context("container absence could not be confirmed"));
                    }
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        };
        tokio::time::timeout(CONTAINER_DISPOSAL_TIMEOUT, confirmation)
            .await
            .map_err(|_| {
                anyhow::anyhow!(
                    "container {container_id} cleanup was accepted, but exact resource absence is unconfirmed"
                )
            })?
    }
}
