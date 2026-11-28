use std::collections::{HashMap, HashSet};
use std::sync::{Arc, OnceLock, PoisonError, Weak};
use std::time::Duration;

use futures::future::OptionFuture;
use tokio::sync::{Mutex, oneshot, watch};
use tokio_util::sync::CancellationToken;
use trogon_sandbox_contract::{
    LeaseError, LeaseId, LeaseRequest, ProviderLease, RunCommandRequest, RunCommandResponse, WorkerId,
};
use trogon_sandbox_protocol::{AuthorityResourceId, AuthorityTaskScope, LifecycleEmitter};
use uuid::Uuid;

use crate::MAX_PERSISTENT_RUNTIMES;
use crate::constants::MAX_EARLY_CANCELS;
use crate::persistent_runtime::wants_persistent_runtime;
use crate::retire_error::RetireError;
use crate::retire_outcome::RetireOutcome;
use crate::run_lease::run_lease;
use crate::session_command::{command_output, session_command_source};
use crate::v8_lease::V8Lease;
use crate::v8_wasm_lease_error::V8WasmLeaseError;

pub(crate) type EmitterFactory = Box<dyn Fn(&LeaseId) -> Arc<dyn LifecycleEmitter> + Send + Sync>;

type AllocationReply = oneshot::Sender<Result<ProviderLease, V8WasmLeaseError>>;

/// Everything a provider shares with the tasks it spawns.
pub(crate) struct ProviderState {
    pub(crate) emitters: EmitterFactory,
    pub(crate) active: Mutex<HashMap<String, Arc<V8Lease>>>,
    pub(crate) known: Mutex<HashMap<String, AuthorityResourceId>>,
    pub(crate) scope: AuthorityTaskScope,
    pub(crate) worker_id: OnceLock<WorkerId>,
    early_cancels: std::sync::Mutex<HashSet<String>>,
    gates: std::sync::Mutex<HashMap<String, Weak<Mutex<()>>>>,
}

impl ProviderState {
    pub(crate) fn new(emitters: EmitterFactory) -> Self {
        Self {
            emitters,
            active: Mutex::default(),
            known: Mutex::default(),
            scope: AuthorityTaskScope::default(),
            worker_id: OnceLock::new(),
            early_cancels: std::sync::Mutex::default(),
            gates: std::sync::Mutex::default(),
        }
    }

    fn lease_gate(&self, lease: &LeaseId) -> Arc<Mutex<()>> {
        let mut gates = self.gates.lock().unwrap_or_else(PoisonError::into_inner);
        gates.retain(|_, gate| gate.strong_count() > 0);
        gates.get(lease.as_str()).and_then(Weak::upgrade).unwrap_or_else(|| {
            let gate = Arc::new(Mutex::new(()));
            gates.insert(lease.as_str().to_owned(), Arc::downgrade(&gate));
            gate
        })
    }

    pub(crate) async fn remember_routed_grant(&self, lease: &LeaseId) {
        let mut known = self.known.lock().await;
        if let Ok(resource) = AuthorityResourceId::new(format!("routed-{}", Uuid::new_v4())) {
            known.entry(lease.as_str().to_owned()).or_insert(resource);
        }
    }

    async fn forget_route(&self, lease: &LeaseId, resource: &AuthorityResourceId) {
        let mut known = self.known.lock().await;
        if known.get(lease.as_str()) == Some(resource) {
            known.remove(lease.as_str());
        }
    }

    async fn remove_active(&self, lease: &LeaseId, held: &Arc<V8Lease>) {
        let mut active = self.active.lock().await;
        if active
            .get(lease.as_str())
            .is_some_and(|current| Arc::ptr_eq(current, held))
        {
            active.remove(lease.as_str());
        }
    }

    /// Takes a lease and starts its guest on a task the caller does not own, so dropping the
    /// returned future never detaches a running guest.
    pub(crate) async fn allocate(
        self: &Arc<Self>,
        request: &LeaseRequest,
        lease_id: &LeaseId,
    ) -> Result<ProviderLease, V8WasmLeaseError> {
        let gate = self.lease_gate(lease_id);
        let held_gate = gate.lock().await;
        let mut active = self.active.lock().await;
        if active.contains_key(lease_id.as_str()) {
            return Err(V8WasmLeaseError::StillRetained);
        }
        let persistent = wants_persistent_runtime(request)?;
        (!persistent || active.values().filter(|held| held.persistent).count() < MAX_PERSISTENT_RUNTIMES)
            .then_some(())
            .ok_or(V8WasmLeaseError::PersistentRuntimeCap(MAX_PERSISTENT_RUNTIMES))?;
        let (resource, completion) = self.scope.track(lease_id.clone(), format!("v8-{}", Uuid::new_v4()))?;
        let (finished, receipt) = watch::channel(false);
        let held = Arc::new(V8Lease {
            resource,
            completion,
            cancel: CancellationToken::new(),
            finished: receipt,
            session: Mutex::new(None),
            persistent,
        });
        let early = self
            .early_cancels
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(lease_id.as_str());
        if early {
            held.cancel.cancel();
        }
        active.insert(lease_id.as_str().to_owned(), Arc::clone(&held));
        drop(active);
        drop(held_gate);
        let (reply, receive) = oneshot::channel();
        tokio::spawn(Arc::clone(self).drive(held, request.clone(), lease_id.clone(), finished, reply));
        receive.await.unwrap_or(Err(V8WasmLeaseError::TaskEnded))
    }

    async fn drive(
        self: Arc<Self>,
        held: Arc<V8Lease>,
        request: LeaseRequest,
        lease: LeaseId,
        finished: watch::Sender<bool>,
        mut reply: AllocationReply,
    ) {
        let emitter = (self.emitters)(&lease);
        let mut session = None;
        let mut result = {
            let running = run_lease(&request, &lease, emitter.as_ref(), held.cancel.clone(), &mut session);
            tokio::pin!(running);
            tokio::select! {
                result = &mut running => result,
                () = reply.closed() => {
                    held.cancel.cancel();
                    running.await
                }
            }
        };
        let keep = result.is_ok() && session.is_some() && !held.cancel.is_cancelled();
        if let Some(session) = session {
            let session = Arc::new(session);
            *held.session.lock().await = Some(Arc::clone(&session));
            if !keep {
                let failure = session.shutdown().await.err();
                result = failure.map_or(result, |error| Err(error.into()));
                held.session.lock().await.take();
            }
        }
        if held.cancel.is_cancelled() {
            result = result.and(Err(V8WasmLeaseError::Cancelled));
        }
        if let Ok(grant) = &mut result {
            grant.worker_id = self.worker_id.get().cloned();
            self.known
                .lock()
                .await
                .insert(lease.as_str().to_owned(), held.resource.clone());
        }
        if !keep {
            held.completion.confirm();
        }
        finished.send(true).ok();
        if !keep {
            self.remove_active(&lease, &held).await;
        }
        let abandoned = reply.send(result).is_err() && keep;
        drop(OptionFuture::from(abandoned.then(|| self.retire(&lease, Some(&held.resource)))).await);
    }

    /// Stops a lease's guest. With `expected`, only the exact resource it names; without, whatever
    /// the lease holds, or a note that a cancel arrived before the lease did.
    pub(crate) async fn retire(
        &self,
        lease: &LeaseId,
        expected: Option<&AuthorityResourceId>,
    ) -> Result<RetireOutcome, RetireError> {
        let gate = self.lease_gate(lease);
        let _held = gate.lock().await;
        if let Some(resource) = expected
            && self.scope.is_completed(lease, resource)
        {
            self.forget_route(lease, resource).await;
            return Ok(RetireOutcome::Settled);
        }
        let held = self.active.lock().await.get(lease.as_str()).cloned();
        let Some(held) = held else {
            return Ok(if expected.is_some() {
                RetireOutcome::Stale
            } else {
                self.note_unheld_cancel(lease).await
            });
        };
        if expected.is_some_and(|resource| &held.resource != resource) {
            return Ok(RetireOutcome::Stale);
        }
        held.cancel.cancel();
        let mut finished = held.finished.clone();
        while !*finished.borrow_and_update() {
            finished.changed().await.map_err(|_| RetireError::NoReceipt)?;
        }
        let session = held.session.lock().await.take();
        let failure = match &session {
            Some(session) => session.shutdown().await.err(),
            None => None,
        };
        held.completion.confirm();
        self.remove_active(lease, &held).await;
        self.forget_route(lease, &held.resource).await;
        failure.map_or(Ok(RetireOutcome::Purged), |error| Err(RetireError::Session(error)))
    }

    async fn note_unheld_cancel(&self, lease: &LeaseId) -> RetireOutcome {
        let was_known = self.known.lock().await.remove(lease.as_str()).is_some();
        let mut early = self.early_cancels.lock().unwrap_or_else(PoisonError::into_inner);
        let full = early.len() >= MAX_EARLY_CANCELS;
        early.retain(|_| !full);
        if !was_known {
            early.insert(lease.as_str().to_owned());
        }
        RetireOutcome::Settled
    }

    pub(crate) async fn shutdown(&self) -> Result<(), RetireError> {
        let leases: Vec<LeaseId> = self.active.lock().await.keys().cloned().map(LeaseId::new).collect();
        for lease in leases {
            self.retire(&lease, None).await?;
        }
        self.known.lock().await.clear();
        Ok(())
    }

    pub(crate) async fn run_session_command(&self, request: &RunCommandRequest) -> Option<RunCommandResponse> {
        let held = self.active.lock().await.get(request.lease_id.as_str()).cloned()?;
        if held.cancel.is_cancelled() {
            return Some(RunCommandResponse::Denied(LeaseError::LeaseNotFound {
                lease_id: request.lease_id.clone(),
            }));
        }
        let session = held.session.lock().await.clone()?;
        let source = match session_command_source(request) {
            Ok(source) => source,
            Err(error) => {
                return Some(RunCommandResponse::Denied(LeaseError::InvalidSpec {
                    reason: error.to_string(),
                }));
            }
        };
        let timeout = Duration::from_secs(u64::from(request.timeout_seconds.max(1)));
        let outcome = session.eval(source, timeout, &held.cancel).await;
        Some(RunCommandResponse::Completed(command_output(
            outcome,
            request.max_output_bytes,
        )))
    }
}
