use super::*;

impl BrokerCore {
    /// Adds to the lease's usage counters. Best effort: a store failure is
    /// logged and never fails the verb being routed.
    pub(super) async fn count_usage(&self, record: &LeaseRecord, delta: LeaseUsageCounters) {
        if let Err(e) = self.usage.add(record, &delta, SystemTime::now()).await {
            warn!(error = %e, lease_id = %record.lease_id, "usage counter update failed");
        }
    }

    /// Records activity on a lease that has an idle timeout, so the sweeper
    /// leaves it alone. Writes are throttled to a tenth of the idle window
    /// (1s to 30s) unless `force`d, so a chatty client does not turn every
    /// verb into a store write. Failures only log: activity tracking must
    /// never fail the verb being routed. Leases placed through a targeted
    /// dispatch attempt are reconciled from the attempt journal, so the stamp
    /// is written there first and the index copy follows.
    /// Counts a worker's activity signal (see
    /// [`trogon_sandbox_protocol::subjects::worker_lease_activity`]) toward the lease's
    /// idle timeout. Only a running lease is touched, and the usual throttle
    /// applies, so a chatty terminal costs at most one write per window.
    pub async fn handle_worker_activity(&self, lease_id: &LeaseId) {
        match self.active_leases.get(lease_id).await {
            Ok(Some(record)) if record.status == LeaseStatus::Running => {
                self.touch_activity(&record, false).await;
            }
            Ok(_) => {}
            Err(e) => {
                warn!(error = %e, lease_id = %lease_id, "worker activity: active lease lookup failed");
            }
        }
    }

    pub(super) async fn touch_activity(&self, record: &LeaseRecord, force: bool) {
        let Some(idle) = crate::idle::idle_stages(&record.spec)
            .iter()
            .map(|stage| stage.after_seconds.get())
            .min()
        else {
            return;
        };
        let now = SystemTime::now();
        let throttle = (idle / 10).clamp(1, 30);
        if !force && !elapsed_at_least(crate::idle::last_activity(record), now, throttle) {
            return;
        }
        if record.dispatch_attempt.is_some() {
            self.stamp_targeted_activity(record, now).await;
        }
        if let Err(e) = self.active_leases.touch(&record.lease_id, now).await {
            warn!(error = %e, lease_id = %record.lease_id, "failed to record lease activity");
        }
    }

    /// Stamps `last_activity_at` on the durable attempt record of a targeted
    /// lease with a compare-and-swap. A lost race means the record moved on
    /// (released, cancelling, or another stamp) and is harmless.
    async fn stamp_targeted_activity(&self, record: &LeaseRecord, now: SystemTime) {
        let Some(attempt) = record.dispatch_attempt.as_ref() else {
            return;
        };
        let current = match self
            .idempotency
            .get_attempt(&record.tenant, &attempt.idempotency_key)
            .await
        {
            Ok(Some(current)) if current.lease_id == record.lease_id && current.status == LeaseStatus::Running => {
                current
            }
            Ok(_) => return,
            Err(e) => {
                warn!(error = %e, lease_id = %record.lease_id, "failed to read attempt for activity stamp");
                return;
            }
        };
        let mut stamped = current.clone();
        stamped.last_activity_at = Some(now);
        match self
            .idempotency
            .replace_attempt(&record.tenant, &attempt.idempotency_key, &current, &stamped)
            .await
        {
            Ok(true) => {}
            Ok(false) => {
                tracing::debug!(lease_id = %record.lease_id, "activity stamp lost a journal race")
            }
            Err(e) => {
                warn!(error = %e, lease_id = %record.lease_id, "failed to stamp attempt activity")
            }
        }
    }
}
