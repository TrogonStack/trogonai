use super::*;

/// Aggregate warm pool population for `(provider, digest)` across live workers.
pub async fn warm_pool_count(registry: &dyn WorkerRegistry, provider: ProviderKind, digest: &Digest) -> u32 {
    registry.warm_pool_count(provider, digest).await
}

/// When aggregate desired pool slots exceed tenant budget, keep highest-popularity digests.
pub fn lru_budget_filter(entries: &[(Digest, PrewarmState)], budget: u32) -> Vec<(Digest, PrewarmState)> {
    let mut sorted: Vec<_> = entries.to_vec();
    sorted.sort_by_key(|entry| std::cmp::Reverse(entry.1.popularity));
    let mut used = 0u32;
    let mut kept = Vec::new();
    for (digest, state) in sorted {
        let slots = state.spec.pool_size.saturating_mul(state.spec.providers.len() as u32);
        if used.saturating_add(slots) <= budget {
            used = used.saturating_add(slots);
            kept.push((digest, state));
        }
    }
    kept
}

pub struct PrewarmController {
    broker: Arc<BrokerCore>,
    prewarm_store: Arc<dyn PrewarmStore>,
    live_snapshots: Arc<dyn LiveSnapshotStore>,
    worker_registry: Arc<dyn WorkerRegistry>,
    snapshot_registry: String,
}

impl PrewarmController {
    pub fn new(
        broker: Arc<BrokerCore>,
        prewarm_store: Arc<dyn PrewarmStore>,
        live_snapshots: Arc<dyn LiveSnapshotStore>,
        worker_registry: Arc<dyn WorkerRegistry>,
        snapshot_registry: String,
    ) -> Self {
        Self {
            broker,
            prewarm_store,
            live_snapshots,
            worker_registry,
            snapshot_registry,
        }
    }

    pub async fn tick(&self) -> Result<u32, PrewarmError> {
        let policies = self.prewarm_store.list_all().await?;
        if policies.is_empty() {
            return Ok(0);
        }

        let mut by_tenant: HashMap<TenantId, Vec<(Digest, PrewarmState)>> = HashMap::new();
        for (tenant, digest, state) in policies {
            by_tenant.entry(tenant).or_default().push((digest, state));
        }

        let mut dispatched = 0u32;
        for (tenant, entries) in by_tenant {
            let budget = tenant_prewarm_budget(&entries);
            let active = lru_budget_filter(&entries, budget);
            for (digest, mut state) in active {
                let Some(live) = self
                    .live_snapshots
                    .get_owned(&tenant, &digest)
                    .await
                    .map_err(|e| PrewarmError::LiveSnapshots(e.into()))?
                else {
                    debug!(tenant = %tenant, digest = %digest, "prewarm skipped: digest not live");
                    continue;
                };
                if live.tenant != tenant {
                    continue;
                }
                let providers = state.spec.providers.clone();
                for provider in providers {
                    let current = warm_pool_count(self.worker_registry.as_ref(), provider, &digest).await;
                    let inflight = state.pending_dispatches;
                    let target = state.spec.pool_size;
                    let shortfall = target.saturating_sub(current.saturating_add(inflight));
                    for _ in 0..shortfall {
                        match self
                            .dispatch_one(&tenant, &digest, live.artifact_type, provider, &mut state)
                            .await
                        {
                            Ok(true) => dispatched = dispatched.saturating_add(1),
                            Ok(false) => break,
                            Err(e) => {
                                warn!(
                                    tenant = %tenant,
                                    digest = %digest,
                                    provider = ?provider,
                                    error = %e,
                                    "prewarm dispatch failed"
                                );
                                break;
                            }
                        }
                    }
                }
                state.updated_at = SystemTime::now();
                self.prewarm_store.put(&tenant, &digest, &state).await?;
            }
        }
        Ok(dispatched)
    }

    async fn dispatch_one(
        &self,
        tenant: &TenantId,
        digest: &Digest,
        artifact_type: ArtifactType,
        provider: ProviderKind,
        state: &mut PrewarmState,
    ) -> Result<bool, PrewarmError> {
        let repository = self
            .broker
            .snapshot_repository_for_restore(tenant, digest, artifact_type)
            .await;
        let req = build_prewarm_request(
            tenant,
            digest,
            artifact_type,
            provider,
            &self.snapshot_registry,
            &repository,
        )?;
        match self.broker.handle_lease_request(req).await {
            LeaseResponse::Granted(_) => {
                state.pending_dispatches = state.pending_dispatches.saturating_add(1);
                state.popularity = state.popularity.saturating_add(1);
                info!(
                    tenant = %tenant,
                    digest = %digest,
                    provider = ?provider,
                    "prewarm lease dispatched"
                );
                Ok(true)
            }
            LeaseResponse::Denied(e) => {
                debug!(
                    tenant = %tenant,
                    digest = %digest,
                    provider = ?provider,
                    error = ?e,
                    "prewarm lease denied"
                );
                Ok(false)
            }
        }
    }
}

fn tenant_prewarm_budget(entries: &[(Digest, PrewarmState)]) -> u32 {
    entries
        .iter()
        .map(|(_, s)| s.spec.pool_size.saturating_mul(s.spec.providers.len() as u32))
        .sum()
}

pub async fn run_prewarm_controller(controller: Arc<PrewarmController>, tick: Duration, cancel: CancellationToken) {
    let mut interval = tokio::time::interval(tick);
    loop {
        tokio::select! {
            _ = cancel.cancelled() => break,
            _ = interval.tick() => {
                if let Err(e) = controller.tick().await {
                    warn!(error = %e, "prewarm controller tick failed");
                }
            }
        }
    }
}
