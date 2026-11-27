use std::sync::Arc;
use std::time::Duration;

use trogon_sandbox_contract::{LeaseId, NetworkPolicy, SandboxSpec};
use trogon_sandbox_providers::{K8sResourceKind, lease_request_secret_name};

use crate::active_k8s_lease::ActiveK8sLease;
use crate::{K8sWorkerError, K8sWorkerProvider, LeaseRecovery};

impl K8sWorkerProvider {
    /// Rebuilds the lease table from the cluster, so a restarted worker can
    /// still serve and cancel the leases it launched before. Lease Jobs whose
    /// lease can no longer run are torn down on the way. Must run before the
    /// worker takes lease requests: a lease launched concurrently would be
    /// recorded twice.
    pub async fn recover_leases(&self) -> Result<LeaseRecovery, K8sWorkerError> {
        let mut recovery = LeaseRecovery::default();
        for found in self.inner.confirmed_lease_inventory().await? {
            let lease_id = LeaseId::new(found.lease_id.clone());
            let _gate = self.authority_gate(&lease_id).lock_owned().await;
            if found.expired {
                match self.inner.release_resources(&found.resources).await {
                    Ok(()) => recovery.swept += 1,
                    Err(error) => {
                        tracing::warn!(lease_id = %lease_id, %error, "retaining expired Kubernetes cleanup receipts");
                        self.cleanup.lock().await.insert(lease_id, found.resources);
                    }
                }
                continue;
            }
            let Some(pod) = found.pod else {
                continue;
            };
            let request = if found.resources.iter().any(|receipt| {
                receipt.kind() == K8sResourceKind::Secret
                    && receipt.name() == lease_request_secret_name(found.owner.name())
            }) {
                self.load_request(found.owner.name()).await
            } else {
                None
            };
            let is_warm_pod = found.owner.kind() == K8sResourceKind::Pod;
            let request_persisted = request.is_some();
            let mut active = self.active.lock().await;
            if active.contains_key(&lease_id) {
                continue;
            }
            if is_warm_pod {
                recovery.warm += 1;
            } else {
                recovery.cold += 1;
            }
            active.insert(
                lease_id,
                ActiveK8sLease {
                    sandbox_id: found.owner.name().to_string(),
                    pod_name: pod.name().to_string(),
                    owner: found.owner,
                    resources: found.resources,
                    is_warm_pod,
                    request_persisted,
                    request,
                },
            );
        }
        Ok(recovery)
    }

    pub async fn sweep_expired_leases(&self) -> Result<usize, K8sWorkerError> {
        let expired = self.inner.confirmed_lease_inventory().await?;
        let mut swept = 0;
        for found in expired.into_iter().filter(|found| found.expired) {
            let lease_id = LeaseId::new(found.lease_id.clone());
            let _gate = self.authority_gate(&lease_id).lock_owned().await;
            if self
                .active
                .lock()
                .await
                .get(&lease_id)
                .is_some_and(|held| held.owner != found.owner)
            {
                continue;
            }
            self.cleanup
                .lock()
                .await
                .entry(lease_id.clone())
                .or_insert(found.resources);
            self.cancel_locked(&lease_id).await?;
            swept += 1;
        }
        let pending: Vec<_> = self.cleanup.lock().await.keys().cloned().collect();
        for lease in pending {
            let _gate = self.authority_gate(&lease).lock_owned().await;
            self.cancel_locked(&lease).await?;
        }
        Ok(swept)
    }

    /// Runs [`Self::sweep_expired_leases`] every `interval` until `cancel`.
    pub fn spawn_lease_sweeper(self: Arc<Self>, interval: Duration, cancel: tokio_util::sync::CancellationToken) {
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(interval);
            ticker.tick().await;
            loop {
                tokio::select! {
                    _ = ticker.tick() => match self.sweep_expired_leases().await {
                        Ok(0) => {}
                        Ok(swept) => tracing::info!(swept, "tore down expired leases"),
                        Err(error) => tracing::warn!(error = %error, "lease job sweep failed"),
                    },
                    _ = cancel.cancelled() => break,
                }
            }
        });
    }

    /// Makes the request kept for a rollback carry `policy`, so a rebuilt
    /// sandbox never starts under a policy the lease no longer has. When the
    /// request is also stored in the cluster, the stored copy follows.
    pub(super) async fn record_policy_in_request(&self, lease_id: &LeaseId, policy: &NetworkPolicy) {
        let updated = {
            let mut active = self.active.lock().await;
            active.get_mut(lease_id).and_then(|lease| {
                let request = lease.request.as_mut()?;
                let current = Arc::make_mut(request);
                let mut parts = current.spec.clone().into_parts();
                parts.network_policy = Some(policy.clone());
                match SandboxSpec::new(parts) {
                    Ok(spec) => current.spec = spec,
                    Err(error) => {
                        tracing::warn!(%lease_id, %error, "could not record the network policy in the lease request");
                        return None;
                    }
                }
                Some((
                    lease.owner.clone(),
                    lease.is_warm_pod,
                    lease.request_persisted,
                    Arc::clone(request),
                ))
            })
        };
        if let Some((owner, _warm, true, request)) = updated
            && let Some(receipt) = self.persist_request(&self.inner, &owner, lease_id, &request).await
            && let Some(active) = self.active.lock().await.get_mut(lease_id)
        {
            active.resources.retain(|held| held != &receipt);
            active.resources.push(receipt);
        }
    }
}
