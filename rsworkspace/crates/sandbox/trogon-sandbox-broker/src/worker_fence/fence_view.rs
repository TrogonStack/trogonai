use std::collections::HashMap;
use std::sync::{Arc, PoisonError};

use tracing::warn;
use trogon_sandbox_contract::{ProviderKind, WorkerId};

use super::{WorkerFence, WorkerFenceStore, WorkerPlacementState};

/// The fences a broker currently honours, shared between a registry (which
/// reads it on every placement) and the task that refreshes it.
#[derive(Clone, Default)]
pub struct FenceView {
    states: Arc<std::sync::Mutex<HashMap<(ProviderKind, WorkerId), WorkerPlacementState>>>,
}

impl FenceView {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn state_of(&self, provider: ProviderKind, worker_id: &WorkerId) -> WorkerPlacementState {
        self.states
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&(provider, worker_id.clone()))
            .copied()
            .unwrap_or_default()
    }

    pub fn admits_placements(&self, provider: ProviderKind, worker_id: &WorkerId) -> bool {
        self.state_of(provider, worker_id).admits_placements()
    }

    pub fn replace(&self, fences: &[WorkerFence]) {
        let next = fences
            .iter()
            .map(|f| ((f.provider, f.worker_id.clone()), f.state))
            .collect();
        *self.states.lock().unwrap_or_else(PoisonError::into_inner) = next;
    }

    /// Reloads the view from `store`. A failed read keeps the last view, so a
    /// KV blip never lifts a cordon.
    pub async fn refresh(&self, store: &dyn WorkerFenceStore) {
        match store.list().await {
            Ok(fences) => self.replace(&fences),
            Err(error) => warn!(%error, "failed to refresh worker fences; keeping last view"),
        }
    }
}
