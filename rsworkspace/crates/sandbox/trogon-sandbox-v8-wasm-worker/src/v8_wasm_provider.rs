use std::sync::Arc;

use async_trait::async_trait;
use trogon_sandbox_contract::{
    LeaseId, LeaseRequest, ProviderKind, ProviderLease, RunCommandRequest, RunCommandResponse, WorkerId,
};
use trogon_sandbox_protocol::lifecycle_subject;
use trogon_sandbox_protocol::{LifecycleEmitter, Provider, ProviderError, reject_unsupported_privileged};

use crate::provider_state::{EmitterFactory, ProviderState};
use crate::retire_error::RetireError;
use crate::worker_binding_error::WorkerBindingError;
use crate::{NatsLifecycleEmitter, PROFILE};

/// Runs a lease's guest, a JS script in a V8 isolate or a Wasm component in Wasmtime, to completion.
///
/// The guest source comes from the lease's host-mounted workspace: `index.js` for JS and
/// `module.wasm` for Wasm. A lease that opts into a persistent runtime keeps its isolate for later
/// `eval` commands until the lease is cancelled.
pub struct V8WasmProvider {
    pub(crate) state: Arc<ProviderState>,
}

impl V8WasmProvider {
    fn build(emitters: EmitterFactory) -> Self {
        Self {
            state: Arc::new(ProviderState::new(emitters)),
        }
    }

    /// A provider that publishes each lease's lifecycle events to its JetStream subject.
    pub fn new(js: async_nats::jetstream::Context) -> Self {
        Self::build(Box::new(move |lease| {
            Arc::new(NatsLifecycleEmitter::new(js.clone(), lifecycle_subject(lease)))
        }))
    }

    /// A provider that hands every lease's lifecycle events to `emitter`.
    pub fn with_emitter(emitter: Arc<dyn LifecycleEmitter>) -> Self {
        Self::build(Box::new(move |_| Arc::clone(&emitter)))
    }

    /// Ties the provider to the worker it serves. Binding the same worker again is allowed.
    pub fn bind_worker(&self, worker: &WorkerId) -> Result<(), WorkerBindingError> {
        (self.state.worker_id.get_or_init(|| worker.clone()) == worker)
            .then_some(())
            .ok_or(WorkerBindingError::AlreadyBound)
    }

    /// How many leases still hold a guest.
    pub async fn active_lease_count(&self) -> usize {
        self.state.active.lock().await.len()
    }

    pub(crate) async fn knows_lease(&self, lease: &LeaseId) -> bool {
        self.state.known.lock().await.contains_key(lease.as_str())
    }

    pub(crate) async fn remember_routed_grant(&self, lease: &LeaseId) {
        self.state.remember_routed_grant(lease).await;
    }

    pub(crate) async fn forget_routed_grant(&self, lease: &LeaseId) {
        self.state.known.lock().await.remove(lease.as_str());
    }

    /// Stops every guest the provider still holds.
    pub async fn shutdown(&self) -> Result<(), RetireError> {
        self.state.shutdown().await
    }

    /// Evaluates the `eval` command of a lease that kept a persistent runtime. `None` when the
    /// lease holds no runtime, so the caller can answer as for any other lease.
    pub async fn run_session_command(&self, request: &RunCommandRequest) -> Option<RunCommandResponse> {
        self.state.run_session_command(request).await
    }
}

#[async_trait]
impl Provider for V8WasmProvider {
    fn kind(&self) -> ProviderKind {
        ProviderKind::V8Wasm
    }

    async fn lease(&self, req: &LeaseRequest, lease_id: &LeaseId) -> Result<ProviderLease, ProviderError> {
        reject_unsupported_privileged(&req.spec, &PROFILE)?;
        self.state.allocate(req, lease_id).await.map_err(ProviderError::backend)
    }

    async fn cancel(&self, lease_id: &LeaseId) -> Result<(), ProviderError> {
        self.state
            .retire(lease_id, None)
            .await
            .map(|_| ())
            .map_err(ProviderError::backend)
    }
}
