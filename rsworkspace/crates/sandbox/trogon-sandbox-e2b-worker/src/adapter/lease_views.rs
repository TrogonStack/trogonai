use trogon_sandbox_contract::{
    GetSandboxDomainResponse, LeaseError, LeaseId, NativeHandle, NativeHandleResponse, ProviderKind,
    SandboxFreezeResponse, SandboxImage,
};

use super::{E2bProviderError, E2bWorkerProvider, LeaseAccess, PauseOwnership};
use crate::LeaseSandbox;
use crate::api::{ApiError, TrafficPrivacy};

impl E2bWorkerProvider {
    pub(super) fn template_for(&self, image: &SandboxImage) -> Result<String, E2bProviderError> {
        match image {
            SandboxImage::Registry { reference, .. } => {
                let template = if reference.repository.is_empty() {
                    self.default_template.clone()
                } else {
                    reference.repository.clone()
                };
                if template.is_empty() {
                    return Err(E2bProviderError::TemplateRequired);
                }
                Ok(template)
            }
            _ => Err(LeaseError::SnapshotUnsupported {
                provider: ProviderKind::E2b,
                reason: "E2B owns its snapshots; name an E2B template as a registry image".to_owned(),
            }
            .into()),
        }
    }

    /// The vendor handle for the sandbox `lease_id` holds on this worker.
    pub async fn native_handle_for_lease(&self, lease_id: &LeaseId) -> NativeHandleResponse {
        let active = self.active.lock().await;
        match active.get(lease_id.as_str()) {
            Some(lease) if lease.access == LeaseAccess::Ready => NativeHandleResponse::Available(NativeHandle {
                provider: ProviderKind::E2b,
                vendor_id: lease.sandbox_id.clone(),
                url: None,
                region: lease.region.clone(),
            }),
            _ => NativeHandleResponse::Denied(LeaseError::LeaseNotFound {
                lease_id: lease_id.clone(),
            }),
        }
    }

    /// The sandbox a lease holds on this worker, or `None` when another worker holds it.
    pub async fn sandbox_for_lease(&self, lease_id: &LeaseId) -> Option<LeaseSandbox> {
        let active = self.active.lock().await;
        let lease = active.get(lease_id.as_str())?;
        if lease.access != LeaseAccess::Ready {
            return None;
        }
        Some(lease.view())
    }

    pub(crate) async fn get_domain_for_lease(
        &self,
        lease_id: &LeaseId,
        admitted: &LeaseSandbox,
        port: u16,
    ) -> GetSandboxDomainResponse {
        let active = self.active.lock().await;
        let lease = match active.get(lease_id.as_str()) {
            Some(lease)
                if lease.access == LeaseAccess::Ready
                    && lease.sandbox_id == admitted.sandbox_id
                    && lease.target.same_instance(&admitted.target) =>
            {
                lease
            }
            _ => {
                return GetSandboxDomainResponse::Denied(LeaseError::LeaseNotFound {
                    lease_id: lease_id.clone(),
                });
            }
        };
        if port != 0
            && lease.traffic_privacy != TrafficPrivacy::Public
            && lease
                .traffic_token
                .as_deref()
                .is_none_or(|token| token.trim().is_empty())
        {
            return GetSandboxDomainResponse::Denied(LeaseError::Backend {
                message: "native preview traffic privacy is protected or unknown and its access credential is \
                          unavailable"
                    .into(),
            });
        }
        self.exec.handle_get_domain(&lease.view(), port)
    }

    /// Pauses (`pause`) or resumes (`!pause`) the sandbox `lease_id` holds. Idempotent. E2B pause keeps memory, so
    /// running processes survive.
    pub async fn set_paused_for_lease(&self, lease_id: &LeaseId, pause: bool) -> SandboxFreezeResponse {
        let backend = |error: ApiError| {
            SandboxFreezeResponse::Denied(LeaseError::Backend {
                message: error.to_string(),
            })
        };
        let gate = self.lease_gate(lease_id);
        let _gate = gate.lock().await;
        let (sandbox_id, lifetime) = {
            let active = self.active.lock().await;
            match active.get(lease_id.as_str()) {
                Some(lease) if lease.access == LeaseAccess::Ready => (lease.sandbox_id.clone(), lease.lifetime),
                _ => {
                    return SandboxFreezeResponse::Denied(LeaseError::LeaseNotFound {
                        lease_id: lease_id.clone(),
                    });
                }
            }
        };
        if pause {
            if let Some(lease) = self.active.lock().await.get_mut(lease_id.as_str()) {
                lease.pause = PauseOwnership::Manual;
            }
            return match self.api.pause_sandbox(&sandbox_id).await {
                Ok(()) | Err(ApiError::Conflict(_)) => SandboxFreezeResponse::Done,
                Err(error) => backend(error),
            };
        }
        match self.resume_lease(lease_id, &sandbox_id, lifetime).await {
            Ok(()) => {
                if let Some(lease) = self.active.lock().await.get_mut(lease_id.as_str()) {
                    lease.pause = PauseOwnership::Unowned;
                }
                SandboxFreezeResponse::Done
            }
            Err(error) => backend(error),
        }
    }

    pub async fn active_lease_count(&self) -> usize {
        self.active.lock().await.len()
    }
}
