use trogon_sandbox_contract::{LeaseError, LeaseId, SandboxFreezeResponse};

use crate::idempotency::{fresh, idempotency_key};
use crate::{LifecycleAction, SandboxState, SpaceId, SpaceProvider, SpaceProviderError};

impl SpaceProvider {
    pub(crate) async fn record(&self, id: &LeaseId) -> Result<crate::LeaseRecord, SpaceProviderError> {
        self.registry
            .lock()
            .await
            .active
            .get(id.as_str())
            .cloned()
            .ok_or(SpaceProviderError::NotHeld)
    }

    /// Pause (memory resident), suspend (durable) or wake a lease. Each is an asynchronous
    /// SPACE operation, so the call returns once the operation has succeeded and the
    /// sandbox is in the target state. Asking for the state the sandbox is already in
    /// succeeds.
    pub async fn set_dormancy(&self, lease: &LeaseId, action: LifecycleAction) -> SandboxFreezeResponse {
        let Ok(record) = self.record(lease).await else {
            return SandboxFreezeResponse::Denied(LeaseError::LeaseNotFound {
                lease_id: lease.clone(),
            });
        };
        match self.transition(&record.sandbox_id, action).await {
            Ok(()) => SandboxFreezeResponse::Done,
            Err(error) => SandboxFreezeResponse::Denied(LeaseError::Backend {
                message: error.to_string(),
            }),
        }
    }

    async fn transition(&self, id: &SpaceId, action: LifecycleAction) -> Result<(), SpaceProviderError> {
        let (target, allowed): (SandboxState, &[SandboxState]) = match action {
            LifecycleAction::Pause => (SandboxState::Paused, &[SandboxState::Running]),
            LifecycleAction::Suspend => (SandboxState::Suspended, &[SandboxState::Running, SandboxState::Paused]),
            LifecycleAction::Wake => (SandboxState::Running, &[SandboxState::Paused, SandboxState::Suspended]),
        };
        let info = self.api.sandbox(id).await?.ok_or(SpaceProviderError::Missing)?;
        if info.state != target {
            if !allowed.contains(&info.state) {
                return Err(SpaceProviderError::CannotTransition {
                    verb: action.verb(),
                    state: info.state,
                });
            }
            let key = idempotency_key(action.verb(), &[id.as_str(), &fresh()]);
            let accepted = self.api.lifecycle(id, action, &key).await?;
            self.wait_operation(&accepted, self.config.operation_timeout, None)
                .await?;
            self.wait_state(id, &target, self.config.operation_timeout, None)
                .await?;
        }
        if action == LifecycleAction::Wake {
            self.api.probe_processes(id, self.config.boot_timeout).await?;
        }
        Ok(())
    }
}
