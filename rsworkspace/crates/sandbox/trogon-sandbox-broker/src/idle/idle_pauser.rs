use std::time::SystemTime;

use async_trait::async_trait;
use trogon_sandbox_contract::{
    LeaseRecord, PauseLeaseRequest, PauseLeaseResponse, ProviderKind, SuspendLeaseRequest, SuspendLeaseResponse,
    SuspendRequestLevel,
};

use super::idle_pause_backoff::IdlePauseBackoff;

/// Lets a sweeper that does not own the lease records pause a lease through
/// the broker's regular pause path.
#[async_trait]
pub trait IdlePauser: Send + Sync {
    /// Whether the provider's capability profile allows pausing.
    async fn can_pause(&self, provider: ProviderKind) -> bool;
    /// Pauses `record` as an idle pause. Returns whether the lease is paused.
    async fn pause_idle(&self, record: &LeaseRecord) -> bool;
    /// Whether `record` can be suspended: its provider offers a suspend level
    /// and the lease allows the artifact. Defaults to no.
    async fn can_suspend(&self, _record: &LeaseRecord) -> bool {
        false
    }
    /// Suspends `record` as an idle suspend. Returns whether the lease is
    /// suspended.
    async fn suspend_idle(&self, _record: &LeaseRecord) -> bool {
        false
    }
}

#[async_trait]
impl IdlePauser for crate::core::BrokerCore {
    async fn can_pause(&self, provider: ProviderKind) -> bool {
        self.provider_can_pause(provider).await
    }

    async fn can_suspend(&self, record: &LeaseRecord) -> bool {
        self.lease_can_suspend(record).await
    }

    async fn suspend_idle(&self, record: &LeaseRecord) -> bool {
        let now = SystemTime::now();
        if !IdlePauseBackoff::due(record.idle_pause_retry.as_ref(), now) {
            return false;
        }
        let suspended = matches!(
            self.handle_suspend_lease(SuspendLeaseRequest {
                operation_id: None,
                tenant: record.tenant.clone(),
                lease_id: record.lease_id.clone(),
                level: SuspendRequestLevel::Auto,
                max_suspended_seconds: None,
            })
            .await,
            SuspendLeaseResponse::Suspended { .. }
        );
        if !suspended {
            self.record_idle_pause_failure(&record.lease_id, now).await;
        }
        suspended
    }

    async fn pause_idle(&self, record: &LeaseRecord) -> bool {
        let now = SystemTime::now();
        if !IdlePauseBackoff::due(record.idle_pause_retry.as_ref(), now) {
            return false;
        }
        let paused = matches!(
            self.pause_lease(
                PauseLeaseRequest {
                    operation_id: None,
                    tenant: record.tenant.clone(),
                    lease_id: record.lease_id.clone(),
                    max_paused_seconds: None,
                },
                true,
            )
            .await,
            PauseLeaseResponse::Paused { .. }
        );
        if !paused {
            self.record_idle_pause_failure(&record.lease_id, now).await;
        }
        paused
    }
}
