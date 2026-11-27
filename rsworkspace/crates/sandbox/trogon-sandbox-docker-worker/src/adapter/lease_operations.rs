use super::*;
use bollard::query_parameters::StatsOptionsBuilder;
use trogon_sandbox_contract::{LeaseError, ResourceUsageResponse, SandboxFreezeResponse};
use trogon_sandbox_protocol::{AuthorityResourceId, HeldLease};

/// Joins a relative `cwd` onto the absolute `base` directory.
pub(super) fn join_relative_cwd(base: &str, cwd: &str) -> String {
    format!("{}/{}", base.trim_end_matches('/'), cwd.trim_start_matches("./"))
}

/// Maps one Docker stats sample to gauges. A figure the daemon does not report stays
/// `None`: the peak is absent on cgroup v2 hosts, and a limit of zero means unlimited.
pub(super) fn gauges_from_stats(
    sample: &bollard::models::ContainerStatsResponse,
) -> trogon_sandbox_contract::ResourceGauges {
    let memory = sample.memory_stats.as_ref();
    trogon_sandbox_contract::ResourceGauges {
        cpu_nanos_total: sample
            .cpu_stats
            .as_ref()
            .and_then(|cpu| cpu.cpu_usage.as_ref())
            .and_then(|usage| usage.total_usage),
        memory_bytes: memory.and_then(|m| m.usage),
        memory_peak_bytes: memory.and_then(|m| m.max_usage),
        memory_limit_bytes: memory.and_then(|m| m.limit).filter(|limit| *limit > 0),
    }
}

impl DockerWorkerProvider {
    pub async fn export_for_lease(
        &self,
        lease_id: &LeaseId,
        target: &SnapshotPushTarget,
    ) -> anyhow::Result<SnapshotReceipt> {
        let _gate = self.authority_gate(lease_id).lock_owned().await;
        let active = self.active_registration(lease_id).await?;
        self.export_container_snapshot(&active.container_id, target).await
    }

    pub(crate) async fn export_container_snapshot(
        &self,
        container_id: &str,
        target: &SnapshotPushTarget,
    ) -> anyhow::Result<SnapshotReceipt> {
        let raw_tar = self
            .inner
            .commit_and_export_rootfs(container_id)
            .await
            .context("commit and export rootfs")?;
        let gz_tar = gzip_tar(&raw_tar)?;
        let registry = if target.registry.is_empty() {
            self.default_registry.as_str()
        } else {
            target.registry.as_str()
        };
        let (artifact_ref, bytes) =
            push_rootfs_artifact(&self.oci_client, registry, &target.repository, gz_tar).await?;
        self.digest_cache.touch(artifact_ref.digest.clone());
        Ok(SnapshotReceipt {
            digest: artifact_ref.digest,
            bytes,
            artifact_type: ArtifactType::SandboxRootfsV1,
        })
    }

    pub async fn checkpoint_for_lease(
        &self,
        lease_id: &LeaseId,
        target: &SnapshotPushTarget,
    ) -> anyhow::Result<SnapshotReceipt> {
        let _gate = self.authority_gate(lease_id).lock_owned().await;
        let active = self.active_registration(lease_id).await?;
        self.checkpoint_container(&active.container_id, target).await
    }

    /// Pushes an updated `NetworkPolicy` to `lease_id`'s running egress
    /// sidecar without restarting the sandbox. Only leases whose container was
    /// launched with a `NetworkPolicy::UserDefined` have a sidecar and thus a
    /// `policy_file_path` to write to; other leases (warm-pool reuse,
    /// checkpoint restore, or `AllowAll`/`DenyAll` launches) have none and
    /// this returns an error.
    pub async fn push_network_policy_for_lease(
        &self,
        lease_id: &LeaseId,
        policy: &NetworkPolicy,
    ) -> anyhow::Result<()> {
        let _gate = self.authority_gate(lease_id).lock_owned().await;
        let active = self.active_registration(lease_id).await?;
        let policy_file_path = active.policy_file_path.ok_or_else(|| {
            anyhow::anyhow!(
                "lease {} has no egress sidecar policy file to update",
                lease_id.as_str()
            )
        })?;
        self.inner
            .push_network_policy(&policy_file_path, policy)
            .await
            .map_err(anyhow::Error::from)
    }

    /// The working directory for a user command. No cwd means the user's
    /// home, or the image's working directory when no user is named. Docker
    /// refuses a relative exec working directory, so a relative cwd is
    /// joined onto that same default, as every other worker does.
    pub(super) async fn exec_cwd(
        &self,
        container_id: &str,
        cwd: Option<String>,
        run_as_user: Option<&SandboxUserName>,
    ) -> anyhow::Result<Option<String>> {
        let home = run_as_user.map(home_dir_for_user);
        let Some(cwd) = cwd else {
            return Ok(home);
        };
        if cwd.starts_with('/') {
            return Ok(Some(cwd));
        }
        let base = match home {
            Some(home) => home,
            None => self
                .docker
                .inspect_container(container_id, None)
                .await?
                .config
                .and_then(|config| config.working_dir)
                .filter(|dir| !dir.is_empty())
                .unwrap_or_else(|| "/".to_string()),
        };
        Ok(Some(join_relative_cwd(&base, &cwd)))
    }

    /// Reports a container that exits after launch on the lease's lifecycle
    /// stream. The watch ends when the lease is released or its container
    /// is replaced.
    pub(super) fn watch_for_exit(
        &self,
        handle: Box<dyn trogon_sandbox_providers::SandboxHandle>,
        lease_id: &LeaseId,
        container_id: String,
        sandbox: SandboxId,
        lifecycle: Arc<NatsLifecycleEmitter>,
    ) {
        let active = Arc::clone(&self.active);
        let lease_id = lease_id.as_str().to_owned();
        tokio::spawn(async move {
            let tracked = || {
                let active = Arc::clone(&active);
                let lease_id = lease_id.clone();
                let container_id = container_id.clone();
                async move {
                    active
                        .lock()
                        .await
                        .get(&lease_id)
                        .is_some_and(|lease| lease.container_id == container_id)
                }
            };
            crate::launch_guard::report_exit(
                handle.as_ref(),
                &sandbox,
                crate::launch_guard::WATCH_POLL,
                tracked,
                lifecycle.as_ref(),
            )
            .await;
        });
    }

    /// Whether the Docker daemon reports the container as paused.
    pub(super) async fn container_is_paused(&self, container_id: &str) -> anyhow::Result<bool> {
        let inspected = self.docker.inspect_container(container_id, None).await?;
        Ok(inspected.state.and_then(|state| state.paused).unwrap_or(false))
    }

    /// Freezes (`pause`) or thaws (`!pause`) `lease_id`'s container with the
    /// cgroup freezer, which keeps files and processes and stops CPU use. The
    /// current state is read first so a repeat succeeds without calling the
    /// daemon again.
    pub async fn set_paused_for_lease(
        &self,
        lease_id: &LeaseId,
        pause: bool,
    ) -> trogon_sandbox_contract::SandboxFreezeResponse {
        let _gate = self.authority_gate(lease_id).lock_owned().await;
        let container_id = match self.active_container_id(lease_id).await {
            Ok(id) => id,
            Err(_) => {
                return SandboxFreezeResponse::Denied(LeaseError::LeaseNotFound {
                    lease_id: lease_id.clone(),
                });
            }
        };
        let resource_id = match AuthorityResourceId::new(container_id.clone()) {
            Ok(resource_id) => resource_id,
            Err(error) => {
                return SandboxFreezeResponse::Denied(LeaseError::Backend {
                    message: error.to_string(),
                });
            }
        };
        self.authority_paused.lock().await.remove(&HeldLease::Provider {
            lease_id: lease_id.clone(),
            provider: ProviderKind::LocalDocker,
            resource_id,
        });
        let outcome = async {
            if self.container_is_paused(&container_id).await? == pause {
                return Ok(());
            }
            if pause {
                self.docker.pause_container(&container_id).await
            } else {
                self.docker.unpause_container(&container_id).await
            }
            .map_err(anyhow::Error::from)
        }
        .await;
        match outcome {
            Ok(()) => SandboxFreezeResponse::Done,
            Err(e) => {
                warn!(lease_id = %lease_id, pause, error = %e, "docker pause/unpause failed");
                SandboxFreezeResponse::Denied(LeaseError::Backend { message: e.to_string() })
            }
        }
    }

    /// One-shot CPU and memory reading from the Docker daemon for `lease_id`'s
    /// container. A figure the daemon does not report stays `None`: the peak
    /// is absent on cgroup v2 hosts, and a limit of zero means unlimited.
    pub async fn resource_usage_for_lease(&self, lease_id: &LeaseId) -> trogon_sandbox_contract::ResourceUsageResponse {
        let container_id = match self.active_container_id(lease_id).await {
            Ok(id) => id,
            Err(_) => {
                return ResourceUsageResponse::Denied(LeaseError::LeaseNotFound {
                    lease_id: lease_id.clone(),
                });
            }
        };
        let options = StatsOptionsBuilder::default().stream(false).one_shot(true).build();
        let mut stats = self.docker.stats(&container_id, Some(options));
        match stats.next().await {
            Some(Ok(sample)) => ResourceUsageResponse::Gauges(gauges_from_stats(&sample)),
            Some(Err(e)) => {
                warn!(lease_id = %lease_id, error = %e, "docker stats failed");
                ResourceUsageResponse::Denied(LeaseError::Backend { message: e.to_string() })
            }
            None => ResourceUsageResponse::Denied(LeaseError::Backend {
                message: "docker returned no stats sample".to_string(),
            }),
        }
    }

    /// The Docker client and container id a terminal for `lease_id` runs in,
    /// or `None` when this worker does not hold the lease.
    pub async fn terminal_target(&self, lease_id: &LeaseId) -> Option<(Docker, String)> {
        let container_id = self.active_container_id(lease_id).await.ok()?;
        Some((self.docker.clone(), container_id))
    }

    pub(super) async fn active_registration(&self, lease_id: &LeaseId) -> anyhow::Result<ActiveDockerLease> {
        let held = self
            .active
            .lock()
            .await
            .get(lease_id.as_str())
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("no active container for lease {}", lease_id))?;
        anyhow::ensure!(
            self.native_registration_admitted(lease_id, &held.registration_id, &held.container_id)
                .await,
            "native lease registration is retiring"
        );
        anyhow::ensure!(
            self.active
                .lock()
                .await
                .get(lease_id.as_str())
                .is_some_and(|current| current.registration_id == held.registration_id
                    && current.container_id == held.container_id),
            "native lease registration changed during admission"
        );
        Ok(held)
    }

    pub(super) async fn active_container_id(&self, lease_id: &LeaseId) -> anyhow::Result<String> {
        Ok(self.active_registration(lease_id).await?.container_id)
    }

    /// Resolves a reachable endpoint for `port` on `lease_id`'s container,
    /// mirroring Vercel Sandbox's `sandbox.domain(port)`.
    pub async fn get_sandbox_domain_for_lease(
        &self,
        lease_id: &LeaseId,
        port: u16,
    ) -> trogon_sandbox_contract::GetSandboxDomainResponse {
        let _gate = self.authority_gate(lease_id).lock_owned().await;
        let available = |endpoint: String| trogon_sandbox_contract::GetSandboxDomainResponse::Available {
            endpoint,
            url: None,
            expires_at: None,
            access: None,
        };
        let container_id = {
            let Ok(lease) = self.active_registration(lease_id).await else {
                return trogon_sandbox_contract::GetSandboxDomainResponse::Denied(
                    trogon_sandbox_contract::LeaseError::LeaseNotFound {
                        lease_id: lease_id.clone(),
                    },
                );
            };
            if let Some(endpoint) = lease.exposed_port_endpoints.get(&port) {
                return available(endpoint.clone());
            }
            lease.container_id.clone()
        };
        if port == 0 {
            return trogon_sandbox_contract::GetSandboxDomainResponse::PortNotExposed;
        }
        // Not published at create: forward it through the container
        // (`supports_expose_after_create`).
        let mut forwards = self.port_forwards.lock().await;
        let lease_forwards = forwards.entry(lease_id.as_str().to_owned()).or_default();
        if let Some(forward) = lease_forwards.get(&port) {
            return available(forward.endpoint.clone());
        }
        if lease_forwards.len() >= crate::port_forward::MAX_FORWARDS_PER_LEASE {
            return trogon_sandbox_contract::GetSandboxDomainResponse::PortNotExposed;
        }
        match crate::port_forward::PortForward::start(self.docker.clone(), container_id, port).await {
            Ok(forward) => {
                // A release that ran while the forward opened already cleared
                // this lease's forwards; dropping it closes the listener.
                if !self.active.lock().await.contains_key(lease_id.as_str()) {
                    forwards.remove(lease_id.as_str());
                    return trogon_sandbox_contract::GetSandboxDomainResponse::Denied(
                        trogon_sandbox_contract::LeaseError::LeaseNotFound {
                            lease_id: lease_id.clone(),
                        },
                    );
                }
                let endpoint = forward.endpoint.clone();
                lease_forwards.insert(port, forward);
                available(endpoint)
            }
            Err(e) => trogon_sandbox_contract::GetSandboxDomainResponse::Denied(
                trogon_sandbox_contract::LeaseError::Backend {
                    message: format!("could not open a forward for port {port}: {e}"),
                },
            ),
        }
    }

    /// The container id behind `lease_id`, which is what the Docker CLI and
    /// API address the sandbox by.
    pub async fn native_handle_for_lease(&self, lease_id: &LeaseId) -> trogon_sandbox_contract::NativeHandleResponse {
        match self.active_registration(lease_id).await {
            Ok(lease) => {
                trogon_sandbox_contract::NativeHandleResponse::Available(trogon_sandbox_contract::NativeHandle {
                    provider: ProviderKind::LocalDocker,
                    vendor_id: lease.container_id.clone(),
                    url: None,
                    region: None,
                })
            }
            Err(_) => trogon_sandbox_contract::NativeHandleResponse::Denied(LeaseError::LeaseNotFound {
                lease_id: lease_id.clone(),
            }),
        }
    }
}
