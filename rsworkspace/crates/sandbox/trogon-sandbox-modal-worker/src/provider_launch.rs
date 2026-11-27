use trogon_sandbox_contract::{LifecycleEvent, ProviderDispatchPayload, ProviderLease, SandboxId, SandboxPhase};
use trogon_sandbox_protocol::{LifecycleEmitter, PhaseTimer, Provider, ProviderError};

use crate::ModalProvider;

impl ModalProvider {
    /// Leases for `dispatch` and reports each step of the allocation as it happens.
    ///
    /// Events that name the lease before Modal has a sandbox for it use the lease id as the sandbox id, and are
    /// skipped when the lease id is not a usable sandbox id.
    pub(crate) async fn launch(
        &self,
        emitter: &dyn LifecycleEmitter,
        dispatch: &ProviderDispatchPayload,
    ) -> Result<ProviderLease, ProviderError> {
        let placeholder = SandboxId::new(dispatch.lease_id.as_str()).ok();
        if let Some(sandbox_id) = &placeholder {
            emitter
                .emit(LifecycleEvent::Started {
                    sandbox_id: sandbox_id.clone(),
                })
                .await;
        }
        let allocate = PhaseTimer::start(SandboxPhase::Allocate);
        emitter.emit(allocate.phase_started()).await;
        let result = self.lease(&dispatch.request, &dispatch.lease_id).await;
        match &result {
            Ok(lease) => {
                emitter.emit(allocate.complete()).await;
                emitter
                    .emit(LifecycleEvent::Completed {
                        sandbox_id: lease.sandbox_id.clone(),
                    })
                    .await;
            }
            Err(error) => {
                emitter.emit(allocate.fail(error.to_string())).await;
                if let Some(sandbox_id) = placeholder {
                    emitter
                        .emit(LifecycleEvent::Failed {
                            sandbox_id,
                            error: error.to_string(),
                        })
                        .await;
                }
            }
        }
        result
    }
}
