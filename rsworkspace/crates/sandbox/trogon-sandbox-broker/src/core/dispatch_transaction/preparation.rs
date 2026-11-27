use super::*;

impl BrokerCore {
    pub(super) async fn ensure_preparing(&self, record: &LeaseRecord) -> Result<(), DispatchTransactionError> {
        let key = &record
            .dispatch_attempt
            .as_ref()
            .ok_or(DispatchTransactionError::MissingAttempt)?
            .idempotency_key;
        let current = self.idempotency.get_attempt(&record.tenant, key).await?;
        if current.as_ref() != Some(record) {
            return Err(DispatchTransactionError::PreparationFenced);
        }
        Ok(())
    }

    pub(super) async fn discard_unstarted(&self, record: &LeaseRecord) -> Result<(), DispatchTransactionError> {
        for mount in record.spec.drives() {
            self.drive_store
                .release_claim(&mount.drive_id, &record.lease_id)
                .await?;
        }
        self.quota
            .release_attempt(&Self::attempt_reservation(record)?, &self.quota_limits())
            .await?;
        let key = &record
            .dispatch_attempt
            .as_ref()
            .ok_or(DispatchTransactionError::MissingAttempt)?
            .idempotency_key;
        let mut released = record.clone();
        released.status = LeaseStatus::Released;
        if let Some(attempt) = released.dispatch_attempt.as_mut() {
            attempt.pending_dispatch = None;
        }
        if self
            .idempotency
            .replace_attempt(&record.tenant, key, record, &released)
            .await?
        {
            self.active_leases.put(&released).await?;
        }
        self.snapshot_pins
            .unpin(&record.lease_id)
            .await
            .map_err(DispatchTransactionError::snapshot_pin)?;
        self.resolved_digests.clear(&record.lease_id).await?;
        self.revoke_credential_policy(&record.tenant, &record.lease_id).await;
        Ok(())
    }
}
