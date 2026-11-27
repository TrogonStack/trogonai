use std::collections::HashMap;
use std::error::Error;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};

use trogon_sandbox_contract::{LeaseId, ProviderKind, WorkerId};
use trogon_sandbox_protocol::{AuthorityResourceId, AuthorityTaskScope, FreezeOutcome, HeldLease, PurgeOutcome};

use super::lease::{Lease, LeaseIncarnation};
use crate::mutex_ext::MutexExt;
use crate::{CommandTable, FrameSink, TerminalHub};

/// What a running worker shares between its tasks.
pub(crate) struct State {
    pub worker_id: WorkerId,
    pub authority_scope: AuthorityTaskScope,
    pub shutting_down: AtomicBool,
    gates: Mutex<HashMap<LeaseId, Weak<tokio::sync::Mutex<()>>>>,
    pub kind: ProviderKind,
    pub commands: CommandTable,
    pub leases: Mutex<HashMap<LeaseId, Arc<Lease>>>,
    pub terminals: TerminalHub,
}

impl State {
    pub(crate) fn new(kind: ProviderKind, commands: CommandTable, worker_id: WorkerId) -> Self {
        Self {
            kind,
            commands,
            worker_id,
            authority_scope: AuthorityTaskScope::default(),
            shutting_down: AtomicBool::new(false),
            gates: Mutex::new(HashMap::new()),
            leases: Mutex::new(HashMap::new()),
            terminals: TerminalHub::default(),
        }
    }

    /// The gate that serializes grants, closes and authority changes of one lease.
    pub(crate) fn gate(&self, id: &LeaseId) -> Arc<tokio::sync::Mutex<()>> {
        let mut gates = self.gates.lock_clean();
        gates.retain(|_, gate| gate.strong_count() > 0);
        let existing = gates.get(id).and_then(Weak::upgrade);
        let gate = existing.unwrap_or_else(|| Arc::new(tokio::sync::Mutex::new(())));
        gates.insert(id.clone(), Arc::downgrade(&gate));
        gate
    }

    /// The lease `id`, when this worker holds it.
    pub(crate) fn lease(&self, id: &LeaseId) -> Option<Arc<Lease>> {
        self.leases.lock_clean().get(id).cloned()
    }

    /// Holds the lease `id`, creating it on the first grant. A repeated grant keeps the lease it
    /// already has.
    pub(crate) async fn grant(&self, id: &LeaseId, region: Option<String>) -> Result<(), Box<dyn Error + Send + Sync>> {
        let _gate = self.gate(id).lock_owned().await;
        if self.shutting_down.load(Ordering::SeqCst) {
            return Err("memory worker is shutting down".into());
        }
        if self.lease(id).is_some() {
            return Ok(());
        }
        let (resource, completion) = self
            .authority_scope
            .track(id.clone(), format!("memory:{}", uuid::Uuid::now_v7()))?;
        let lease = Lease::new(id.clone(), region, LeaseIncarnation { resource, completion });
        self.leases.lock_clean().insert(id.clone(), Arc::new(lease));
        Ok(())
    }

    pub(crate) async fn close_lease(&self, sink: &dyn FrameSink, id: &LeaseId) {
        let _gate = self.gate(id).lock_owned().await;
        self.close_lease_locked(sink, id).await;
    }

    async fn close_lease_locked(&self, sink: &dyn FrameSink, id: &LeaseId) {
        let Some(lease) = self.lease(id) else {
            return;
        };
        let tasks = {
            let _admission = lease.admission.lock().await;
            lease.closing.store(true, Ordering::SeqCst);
            lease.tasks.lock_clean().values().cloned().collect::<Vec<_>>()
        };
        for task in &tasks {
            task.request_close();
        }
        for task in tasks {
            task.join().await;
        }
        self.terminals.close_lease(sink, id).await;
        lease.tasks.lock_clean().clear();
        lease.execs.lock_clean().clear();
        lease.procs.lock_clean().clear();
        lease.launches.lock_clean().clear();
        lease.incarnation.completion.confirm();
        self.leases.lock_clean().remove(id);
    }

    /// The leases this worker holds, as the authority guard sees them.
    pub(crate) fn held_leases(&self) -> Vec<HeldLease> {
        self.leases
            .lock_clean()
            .iter()
            .map(|(id, lease)| HeldLease::Provider {
                lease_id: id.clone(),
                provider: self.kind,
                resource_id: lease.incarnation.resource.clone(),
            })
            .collect()
    }

    fn expected<'a>(&self, lease: &'a HeldLease) -> Option<(&'a LeaseId, &'a AuthorityResourceId)> {
        match lease {
            HeldLease::Provider {
                lease_id,
                provider,
                resource_id,
            } if *provider == self.kind => Some((lease_id, resource_id)),
            _ => None,
        }
    }

    fn holds(&self, id: &LeaseId, resource: &AuthorityResourceId) -> bool {
        self.lease(id)
            .is_some_and(|lease| lease.incarnation.resource == *resource)
    }

    /// Memory leases cannot be frozen by the guard, so a held lease reports Unsupported.
    pub(crate) async fn freeze(&self, lease: &HeldLease) -> FreezeOutcome {
        let Some((id, resource)) = self.expected(lease) else {
            return FreezeOutcome::Stale;
        };
        let _gate = self.gate(id).lock_owned().await;
        if self.holds(id, resource) {
            FreezeOutcome::Unsupported
        } else {
            FreezeOutcome::Stale
        }
    }

    /// Closes the lease when it is the exact incarnation `lease` names.
    pub(crate) async fn purge(&self, sink: &dyn FrameSink, lease: &HeldLease) -> PurgeOutcome {
        let Some((id, resource)) = self.expected(lease) else {
            return PurgeOutcome::Stale;
        };
        let _gate = self.gate(id).lock_owned().await;
        if self.holds(id, resource) {
            self.close_lease_locked(sink, id).await;
            PurgeOutcome::Purged
        } else if self.authority_scope.is_completed(id, resource) {
            PurgeOutcome::Settled
        } else {
            PurgeOutcome::Stale
        }
    }
}
