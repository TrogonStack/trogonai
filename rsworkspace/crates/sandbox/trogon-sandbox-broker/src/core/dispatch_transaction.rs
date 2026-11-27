use std::collections::HashMap;

use trogon_sandbox_contract::{
    DispatchTarget, DriveId, DriveRecord, LeaseAccounting, LeaseDispatchAttempt, LeaseRequestKey,
    ProviderCleanupRequest, ProviderCleanupResponse,
};

use super::dispatch_transaction_error::DispatchTransactionError;
use super::lock::lock;
use super::*;
use crate::quota::{AttemptQuotaLimits, AttemptQuotaReservation};

/// How a re-sent dispatch ended. See [`BrokerCore::send_stored_dispatch`].
pub(super) enum StoredDispatchOutcome {
    /// The worker answered with the attempt's receipt and Running is committed.
    Granted(LeaseGranted),
    /// The provider refused the dispatch; the attempt's teardown was started.
    Refused(LeaseError),
    /// No verdict yet, or the commit did not land; the attempt stays Pending.
    Unresolved(LeaseError),
}

pub(super) struct TargetedAdmission {
    pub entity_id: Option<trogon_sandbox_contract::SandboxEntityId>,
    pub lease_id: LeaseId,
    pub provider: ProviderKind,
    pub target: DispatchTarget,
    /// The target worker advertised `supports_lease_epoch`, so the minted
    /// epoch is stamped on its dispatch and signed requests.
    pub epoch_capable: bool,
    pub request: LeaseRequest,
    pub dispatch: LeaseRequest,
    pub drive_records: HashMap<DriveId, DriveRecord>,
    pub correlation_id: CorrelationId,
    pub deadline: SystemTime,
    /// Set when a durable operation drives this create: the payload is stored
    /// on it before the send so a re-drive can resend it.
    pub operation: Option<super::create_operation::OperationHandle>,
}

mod cancel;
mod dispatch;
mod preparation;
mod reconciliation;
mod redrive;
mod reply;

impl BrokerCore {
    pub(super) fn attempt_lock(&self, lease: &LeaseId) -> Arc<tokio::sync::Mutex<()>> {
        let mut locks = lock(&self.attempt_locks);
        locks.retain(|_, lock| lock.strong_count() > 0);
        if let Some(lock) = locks.get(lease.as_str()).and_then(std::sync::Weak::upgrade) {
            return lock;
        }
        let lock = Arc::new(tokio::sync::Mutex::new(()));
        locks.insert(lease.as_str().to_owned(), Arc::downgrade(&lock));
        lock
    }

    pub(super) fn quota_limits(&self) -> AttemptQuotaLimits {
        AttemptQuotaLimits {
            cpu: self.config.default_cpu_cap,
            wall: self.config.default_wall_cap,
            active: self.config.default_active_lease_cap,
            max_lease_lifetime_seconds: self.lifetime.max_lease_lifetime_seconds,
        }
    }

    pub(super) fn pending_redrives(&self) -> std::sync::MutexGuard<'_, HashMap<String, PendingRedrive>> {
        lock(&self.pending_redrives)
    }

    pub(super) fn attempt_reservation(
        record: &LeaseRecord,
    ) -> Result<AttemptQuotaReservation, DispatchTransactionError> {
        let attempt = record
            .dispatch_attempt
            .as_ref()
            .ok_or(DispatchTransactionError::MissingAttempt)?;
        Ok(AttemptQuotaReservation {
            tenant: record.tenant.clone(),
            attempt_id: attempt.id.clone(),
            created_at: record.created_at,
            cpu_seconds: record.cpu_seconds_consumed,
            wall_seconds: record.wall_seconds_consumed,
            accounting: attempt.accounting,
        })
    }

    pub(super) async fn update_record(
        &self,
        original: &LeaseRecord,
        updated: &LeaseRecord,
    ) -> Result<(), DispatchTransactionError> {
        if let Some(attempt) = &original.dispatch_attempt {
            let replaced = self
                .idempotency
                .replace_attempt(&original.tenant, &attempt.idempotency_key, original, updated)
                .await?;
            if !replaced {
                return Err(DispatchTransactionError::LeaseChanged);
            }
        }
        self.active_leases.put(updated).await?;
        Ok(())
    }

    pub(super) async fn refund_initial_quota(
        &self,
        reserved: bool,
        tenant: &TenantId,
        cpu: u32,
        wall: u32,
        ctx: &mut CreateCtx,
    ) {
        if reserved {
            if ctx.quota_reservation.is_some() {
                if ctx.dispatch_unknown || ctx.quota_retry {
                    return;
                }
                ctx.quota_release_requested = true;
            } else {
                let _ = self.quota.release(tenant, cpu, wall).await;
            }
        }
    }

    pub(super) async fn attempt_response(&self, record: LeaseRecord) -> LeaseResponse {
        if let Err(error) = self.active_leases.put(&record).await {
            return LeaseResponse::Denied(LeaseError::Backend {
                message: format!("dispatch journal index recovery failed: {error}"),
            });
        }
        if record.status == LeaseStatus::Running {
            return LeaseResponse::Granted(LeaseGranted {
                entity_id: record.entity_id.clone(),
                operation_id: None,
                lease_id: record.lease_id.clone(),
                provider: record.provider,
                sandbox_id: record.sandbox_id,
                evidence_subject: subjects::evidence_subject(&record.lease_id),
                deadline: record.deadline,
                created: LeaseCreation::Existing,
                capabilities: Vec::new(),
            });
        }
        LeaseResponse::Denied(LeaseError::Backend {
            message: format!(
                "dispatch attempt for lease {} is {:?}; inspect or cancel that lease before retrying",
                record.lease_id.as_str(),
                record.status
            ),
        })
    }
}
