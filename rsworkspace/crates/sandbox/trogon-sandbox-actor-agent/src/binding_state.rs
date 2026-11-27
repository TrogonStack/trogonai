use std::num::NonZeroU16;

use tokio::sync::RwLock;
use trogon_sandbox_contract::LeaseId;

use crate::{BindingError, LeaseBinding};

/// The lease an actor currently serves, replaced on every bind.
#[derive(Debug, Default)]
pub struct BindingState {
    current: RwLock<Option<LeaseBinding>>,
}

impl BindingState {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn bind(&self, binding: LeaseBinding) {
        *self.current.write().await = Some(binding);
    }

    /// Resolves the binding for a call that claims to belong to `lease_id`.
    ///
    /// A mismatch is a refusal rather than a best-effort execution: an actor is reused across a suspend and resume
    /// cycle, so a request carrying a stale lease is a request whose sandbox no longer exists.
    pub async fn resolve(&self, lease_id: &LeaseId) -> Result<LeaseBinding, BindingError> {
        let guard = self.current.read().await;
        match guard.as_ref() {
            None => Err(BindingError::Unbound),
            Some(binding) if &binding.lease_id == lease_id => Ok(binding.clone()),
            Some(binding) => Err(BindingError::LeaseMismatch {
                bound: binding.lease_id.clone(),
                requested: lease_id.clone(),
            }),
        }
    }

    pub async fn exposed_port(&self) -> Option<NonZeroU16> {
        self.current
            .read()
            .await
            .as_ref()
            .and_then(|binding| binding.exposed_port)
    }
}
