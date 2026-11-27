use super::*;

impl BrokerCore {
    pub async fn reconcile_expired_leases(&self) {
        let records = match self.active_leases.all().await {
            Ok(records) => records,
            Err(error) => {
                warn!(%error, "lease reconciliation listing failed");
                return;
            }
        };
        for indexed in records {
            let Some(attempt) = indexed.dispatch_attempt.as_ref() else {
                continue;
            };
            let record = match self
                .idempotency
                .get_attempt(&indexed.tenant, &attempt.idempotency_key)
                .await
            {
                Ok(Some(current))
                    if current.lease_id == indexed.lease_id
                        && current.worker_id == indexed.worker_id
                        && current.dispatch_attempt.as_ref().is_some_and(|durable| {
                            durable.id == attempt.id
                                && durable.idempotency_key == attempt.idempotency_key
                                && durable.accounting == attempt.accounting
                        }) =>
                {
                    current
                }
                Err(error) => {
                    warn!(lease_id = %indexed.lease_id, %error, "lease reconciliation journal unavailable");
                    continue;
                }
                _ => {
                    warn!(lease_id = %indexed.lease_id, "lease reconciliation has no matching durable attempt");
                    continue;
                }
            };
            if indexed != record
                && let Err(error) = self.active_leases.put(&record).await
            {
                warn!(%error, "lease index reconciliation failed");
            }
            if record.status == LeaseStatus::Released {
                // A delayed claim can commit after cleanup if its admitting broker crashes.
                for mount in record.spec.drives() {
                    if let Err(error) = self.drive_store.release_claim(&mount.drive_id, &record.lease_id).await {
                        warn!(lease_id = %record.lease_id, %error, "terminal drive claim reconciliation failed");
                    }
                }
                if let Err(error) = self.snapshot_pins.unpin(&record.lease_id).await {
                    warn!(%error, "terminal snapshot unpin failed");
                }
                if let Err(error) = self.resolved_digests.clear(&record.lease_id).await {
                    warn!(%error, "terminal digest cleanup failed");
                }
                continue;
            }
            if record.status != LeaseStatus::Pending {
                self.pending_redrives().remove(record.lease_id.as_str());
            } else if record.deadline > SystemTime::now() {
                self.redrive_pending(&record).await;
                continue;
            }
            let now = SystemTime::now();
            let caps = crate::idle::idle_capabilities(&record, self).await;
            let idle_action = crate::idle::next_idle_action_with(&record, now, caps);
            if record.deadline > now {
                match idle_action {
                    Some(crate::idle::IdleAction::Pause) => {
                        crate::idle::IdlePauser::pause_idle(self, &record).await;
                        continue;
                    }
                    Some(crate::idle::IdleAction::Suspend) => {
                        crate::idle::IdlePauser::suspend_idle(self, &record).await;
                        continue;
                    }
                    _ => {}
                }
            }
            let idle = idle_action == Some(crate::idle::IdleAction::Release);
            if record.deadline > now && record.status != LeaseStatus::Cancelling && !idle {
                continue;
            }
            let reason = if record.deadline <= now {
                LeaseReleasedReason::Expired
            } else if idle && record.status != LeaseStatus::Cancelling {
                LeaseReleasedReason::IdleTimeout
            } else {
                LeaseReleasedReason::Cancelled
            };
            if let Err(error) = self.cancel_targeted(&record, CancelIntent::Release, reason).await {
                warn!(lease_id = %record.lease_id, %error, "targeted lease cleanup remains pending");
            }
        }
    }
}
