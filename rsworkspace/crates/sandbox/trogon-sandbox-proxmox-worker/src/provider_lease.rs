use std::collections::HashSet;

use trogon_sandbox_contract::{
    LeaseAttemptIdentity, LeaseId, LeaseRequest, NetworkPolicy, ProviderDispatchPayload, ProviderKind, ProviderLease,
};

use crate::check::ensure;
use crate::constants::{DEFAULT_CWD, NO_PROXY, PROXY_ENV_KEYS, VM_NAME_PREFIX};
use crate::provider_journal::read_journal;
use crate::provider_template::ensure_clonable;
use crate::{LeaseProxy, LeaseRecord, ProxmoxProvider, ProxmoxProviderError, VmPhase};

fn needs_network(request: &LeaseRequest) -> bool {
    !matches!(request.spec.network_policy(), None | Some(NetworkPolicy::DenyAll))
}

impl ProxmoxProvider {
    /// Runs a dispatch that carries the assignment the broker made, through the durable ledger.
    pub async fn lease_dispatch(
        &self,
        dispatch: &ProviderDispatchPayload,
    ) -> Result<ProviderLease, ProxmoxProviderError> {
        self.validate_lease(&dispatch.request)?;
        let Some(target) = dispatch.target.clone() else {
            return self.lease_inner(&dispatch.request, &dispatch.lease_id, None).await;
        };
        let identity = LeaseAttemptIdentity {
            lease_id: dispatch.lease_id.clone(),
            provider: ProviderKind::Proxmox,
            target,
        };
        self.ledger
            .dispatch(&identity, || {
                self.lease_inner(&dispatch.request, &dispatch.lease_id, Some(&identity))
            })
            .await
            .map_err(ProxmoxProviderError::from)
    }

    async fn free_network_slot(&self) -> Result<usize, ProxmoxProviderError> {
        let used: HashSet<usize> = self
            .active
            .lock()
            .await
            .values()
            .filter_map(|record| record.network_slot)
            .collect();
        (0..self.config.max_leases)
            .find(|slot| !used.contains(slot))
            .ok_or(ProxmoxProviderError::Backend("guest address pool exhausted"))
    }

    pub(crate) async fn lease_inner(
        &self,
        request: &LeaseRequest,
        lease: &LeaseId,
        identity: Option<&LeaseAttemptIdentity>,
    ) -> Result<ProviderLease, ProxmoxProviderError> {
        self.validate_lease(request)?;
        self.journal_path(lease)?;
        let _allocation = self.allocation.lock().await;
        if let Some(identity) = identity {
            ensure(
                identity.provider == ProviderKind::Proxmox && identity.lease_id == *lease,
                ProxmoxProviderError::Rejected("invalid VM assignment"),
            )?;
            ensure(
                read_journal(&self.attempt_path(identity)).await?.is_none(),
                ProxmoxProviderError::Backend("VM allocation assignment already has a journal"),
            )?;
        }
        let free = !self.holds_lease(lease).await && self.current_leases().await < self.config.max_leases;
        ensure(free, ProxmoxProviderError::Capacity)?;
        ensure_clonable(&self.api.config(self.config.template).await?)?;
        let network_slot = if needs_network(request) {
            Some(self.free_network_slot().await?)
        } else {
            None
        };
        let mut proxy = match (&self.config.network, network_slot, request.spec.network_policy()) {
            (Some(network), Some(_), Some(policy)) => Some(LeaseProxy::start(network, policy.clone()).await?),
            _ => None,
        };
        let vm = self.api.next_id().await?;
        let mut record = LeaseRecord {
            vm,
            name: format!("{VM_NAME_PREFIX}{}", uuid::Uuid::new_v4().simple()),
            cwd: request.spec.working_dir().unwrap_or(DEFAULT_CWD).to_owned(),
            env: request.spec.env().clone(),
            network_slot,
            phase: VmPhase::Unknown,
            assignment: identity.cloned(),
        };
        let saved = match ensure(
            vm != self.config.template,
            ProxmoxProviderError::Backend("allocation returned the template ID"),
        ) {
            Ok(()) => self.save(lease, &record).await,
            Err(error) => Err(error),
        };
        if let Err(error) = saved {
            if let Some(proxy) = proxy.take() {
                let _ = proxy.shutdown().await;
            }
            return Err(error);
        }
        if let Some(proxy) = &proxy {
            for key in PROXY_ENV_KEYS {
                record.env.insert(key.to_owned(), proxy.url.clone());
            }
            record.env.insert("NO_PROXY".to_owned(), NO_PROXY.to_owned());
            record.env.insert("no_proxy".to_owned(), NO_PROXY.to_owned());
        }
        match self.boot(request, lease, &mut record, proxy.as_ref()).await {
            Ok(()) => {
                self.active.lock().await.insert(lease.clone(), record.clone());
                if let Some(proxy) = proxy.take() {
                    self.proxies.lock().await.insert(lease.clone(), proxy);
                }
            }
            Err(error) => {
                if let Some(proxy) = proxy.take() {
                    let _ = proxy.shutdown().await;
                }
                if let Err(cleanup) = self.destroy_owned(lease, &record).await {
                    self.active.lock().await.insert(lease.clone(), record);
                    self.pending_cleanup.lock().await.push_back(lease.clone());
                    return Err(ProxmoxProviderError::CleanupPending {
                        launch: Box::new(error),
                        cleanup: Box::new(cleanup),
                    });
                }
                self.remove_journal(lease).await?;
                return Err(error);
            }
        }
        Self::receipt(vm, lease)
    }
}
