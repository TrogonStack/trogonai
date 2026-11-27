use super::*;

impl BrokerCore {
    pub(in crate::core) async fn cancel_targeted(
        &self,
        original: &LeaseRecord,
        intent: CancelIntent,
        reason: LeaseReleasedReason,
    ) -> Result<(), DispatchTransactionError> {
        let _cancelling = self.attempt_lock(&original.lease_id).lock_owned().await;
        let attempt = original
            .dispatch_attempt
            .as_ref()
            .ok_or(DispatchTransactionError::MissingAttempt)?;
        let key = &attempt.idempotency_key;
        let mut record = self
            .idempotency
            .get_attempt(&original.tenant, key)
            .await?
            .unwrap_or_else(|| original.clone());
        let same_attempt = record.lease_id == original.lease_id
            && record.worker_id == original.worker_id
            && record.dispatch_attempt.as_ref().is_some_and(|current| {
                current.id == attempt.id
                    && current.idempotency_key == attempt.idempotency_key
                    && current.accounting == attempt.accounting
            });
        if !same_attempt {
            return Err(DispatchTransactionError::CleanupIdentityChanged);
        }
        let was_running = !record.sandbox_id.is_empty();
        let already_released = record.status == LeaseStatus::Released;
        let persisted_reason = record
            .dispatch_attempt
            .as_ref()
            .and_then(|attempt| attempt.cleanup_reason);
        let reason = persisted_reason.unwrap_or(reason);
        if !already_released
            && (record.status != LeaseStatus::Cancelling
                || persisted_reason.is_none()
                || record
                    .dispatch_attempt
                    .as_ref()
                    .is_none_or(|current| current.cleanup_started_at.is_none()))
        {
            let mut cancelling = record.clone();
            cancelling.status = LeaseStatus::Cancelling;
            let cancelling_attempt = cancelling
                .dispatch_attempt
                .as_mut()
                .ok_or(DispatchTransactionError::MissingAttempt)?;
            cancelling_attempt.cleanup_reason = Some(reason);
            cancelling_attempt
                .cleanup_started_at
                .get_or_insert_with(SystemTime::now);
            // The journaled dispatch only serves redispatch, which a cancelled
            // or refused attempt never gets; it can hold the request's spec.
            cancelling_attempt.pending_dispatch = None;
            let transitioned = self
                .idempotency
                .replace_attempt(&record.tenant, key, &record, &cancelling)
                .await?;
            if !transitioned {
                return Err(DispatchTransactionError::CleanupTransitionLost);
            }
            record = cancelling;
            self.active_leases.put(&record).await?;
        }
        let identity = trogon_sandbox_contract::LeaseAttemptIdentity {
            lease_id: record.lease_id.clone(),
            provider: record.provider,
            target: DispatchTarget {
                worker_id: record
                    .worker_id
                    .clone()
                    .ok_or(DispatchTransactionError::MissingTargetWorker)?,
                attempt_id: attempt.id.clone(),
                epoch: attempt.epoch,
            },
        };
        let request = ProviderCleanupRequest {
            identity: identity.clone(),
            intent,
        };
        let payload = Bytes::from(serde_json::to_vec(&request)?);
        let reply = self
            .bus
            .request_with_timeout(
                &subjects::worker_cleanup(record.provider, &identity.target.worker_id),
                payload,
                Duration::from_secs(30),
            )
            .await
            .map_err(DispatchTransactionError::bus)?;
        let receipt: ProviderCleanupResponse = serde_json::from_slice(&reply)?;
        let confirmed = matches!(
            receipt,
            ProviderCleanupResponse::Cleaned { identity: ref confirmed, intent: confirmed_intent }
                if confirmed == &identity && confirmed_intent == intent
        );
        if !confirmed {
            return Err(DispatchTransactionError::TeardownUnconfirmed);
        }
        for mount in record.spec.drives() {
            self.drive_store
                .release_claim(&mount.drive_id, &record.lease_id)
                .await?;
        }
        let refunded = if already_released {
            false
        } else {
            self.quota
                .release_attempt(&Self::attempt_reservation(&record)?, &self.quota_limits())
                .await?
        };
        self.snapshot_pins
            .unpin(&record.lease_id)
            .await
            .map_err(DispatchTransactionError::snapshot_pin)?;
        self.resolved_digests.clear(&record.lease_id).await?;
        // Every path that ends a targeted lease lands here, including the
        // reconciler's deadline and idle expiry, so its launch keys and
        // unmonitored history go with it.
        self.purge_lease_command_state(&record).await;
        if already_released {
            return Ok(());
        }
        self.entity_run_ended(&record, &format!("{reason:?}")).await;
        let event = LeaseReleased {
            lease_id: record.lease_id.clone(),
            tenant: record.tenant.clone(),
            reason,
            snapshot_out: if was_running && intent == CancelIntent::Release {
                record.spec.snapshot_out().cloned()
            } else {
                None
            },
            worker_id: record.worker_id.clone(),
            correlation_id: record.correlation_id.clone(),
            restored_from: record.restored_from.clone(),
            released_at: record
                .dispatch_attempt
                .as_ref()
                .and_then(|attempt| attempt.cleanup_started_at),
        };
        self.bus
            .publish_with_msg_id(
                &subjects::lease_released(&record.lease_id),
                &super::lease_released_msg_id(&record.lease_id),
                Bytes::from(serde_json::to_vec(&event)?),
            )
            .await
            .map_err(DispatchTransactionError::bus)?;
        let mut released = record.clone();
        released.status = LeaseStatus::Released;
        let completed = self
            .idempotency
            .replace_attempt(&record.tenant, key, &record, &released)
            .await?;
        if !completed {
            return Err(DispatchTransactionError::CleanupCompletionLost);
        }
        self.active_leases.put(&released).await?;
        self.revoke_credential_policy(&record.tenant, &record.lease_id).await;
        if refunded && was_running {
            self.metrics.dec_active_leases(record.tenant.as_str());
        }
        Ok(())
    }
}
