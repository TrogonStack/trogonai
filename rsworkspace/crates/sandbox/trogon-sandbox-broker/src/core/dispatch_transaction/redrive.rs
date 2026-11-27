use super::*;

impl BrokerCore {
    /// Decides whether this broker re-sends the Pending attempt now, and what.
    /// Replicas share the journaled dispatch, so the re-send is claimed by a
    /// compare-and-swap that moves `sent_at` forward: the replica that wins
    /// sends, the others see a fresh `sent_at` and wait out the timeout again.
    /// A dispatch whose journal write failed can only be re-sent by the
    /// process that holds it in memory.
    async fn claim_redrive(&self, record: &LeaseRecord) -> Option<(Bytes, LeaseRecord)> {
        let attempt = record.dispatch_attempt.as_ref()?;
        let timeout = self.config.provider_dispatch_timeout;
        let now = SystemTime::now();
        let local = self
            .pending_redrives()
            .get(record.lease_id.as_str())
            .map(|entry| (entry.last_sent, entry.payload.clone()));
        if let Some((last_sent, _)) = &local
            && now < *last_sent + timeout
        {
            return None;
        }
        let current = match self
            .idempotency
            .get_attempt(&record.tenant, &attempt.idempotency_key)
            .await
        {
            Ok(Some(current)) if current.lease_id == record.lease_id && current.status == LeaseStatus::Pending => {
                current
            }
            Ok(_) => return None,
            Err(error) => {
                warn!(lease_id = %record.lease_id, %error, "redrive attempt lookup failed");
                return None;
            }
        };
        let journaled = current
            .dispatch_attempt
            .as_ref()
            .and_then(|attempt| attempt.pending_dispatch.clone());
        let (payload, current) = match (journaled, local) {
            (Some(pending), local) => {
                if now < pending.sent_at + timeout {
                    return None;
                }
                let mut claimed = current.clone();
                if let Some(attempt) = claimed.dispatch_attempt.as_mut() {
                    attempt.pending_dispatch = Some(trogon_sandbox_contract::PendingDispatch {
                        sent_at: now,
                        payload: pending.payload.clone(),
                    });
                }
                match self
                    .idempotency
                    .replace_attempt(&record.tenant, &attempt.idempotency_key, &current, &claimed)
                    .await
                {
                    Ok(true) => {}
                    Ok(false) => return None,
                    Err(error) => {
                        warn!(lease_id = %record.lease_id, %error, "redrive claim failed");
                        return None;
                    }
                }
                let payload = match local {
                    Some((_, payload)) => payload,
                    None => Bytes::from(serde_json::to_vec(&pending.payload).ok()?),
                };
                (payload, claimed)
            }
            (None, Some((_, payload))) => (payload, current),
            (None, None) => return None,
        };
        self.pending_redrives().insert(
            record.lease_id.as_str().to_owned(),
            PendingRedrive {
                payload: payload.clone(),
                last_sent: now,
            },
        );
        Some((payload, current))
    }

    /// Re-sends a Pending attempt whose dispatch reply was lost. The worker
    /// dedupes by lease and attempt id (`AttemptLedger::dispatch` returns the
    /// stored receipt of a Ready attempt and refuses anything else), so this
    /// never launches a second sandbox. A matching receipt commits Running.
    pub(in crate::core) async fn redrive_pending(&self, record: &LeaseRecord) {
        if record.dispatch_attempt.is_none() || record.worker_id.is_none() {
            return;
        }
        let Some((payload, claimed)) = self.claim_redrive(record).await else {
            return;
        };
        // Later writes compare against the journal as this claim left it.
        match self.send_stored_dispatch(&claimed, payload).await {
            StoredDispatchOutcome::Granted(_) => {}
            StoredDispatchOutcome::Refused(_) => {}
            StoredDispatchOutcome::Unresolved(error) => {
                warn!(lease_id = %claimed.lease_id, %error, "re-driven dispatch is still unresolved");
            }
        }
    }

    /// Sends a stored dispatch payload to the worker its attempt targets and
    /// settles the attempt from the reply. The one place a re-sent dispatch
    /// is judged, whether the in-memory redrive or a durable create operation
    /// sent it: a matching receipt commits Running, a provider refusal tears
    /// the attempt down, anything else leaves it for the next try.
    pub(in crate::core) async fn send_stored_dispatch(
        &self,
        record: &LeaseRecord,
        payload: Bytes,
    ) -> StoredDispatchOutcome {
        let (Some(attempt), Some(worker_id)) = (record.dispatch_attempt.as_ref(), record.worker_id.as_ref()) else {
            return StoredDispatchOutcome::Unresolved(LeaseError::Backend {
                message: format!("lease {} has no dispatch attempt to re-send", record.lease_id.as_str()),
            });
        };
        let decoded = self.request_dispatch_receipt(record.provider, worker_id, payload).await;
        match decoded {
            Ok(receipt)
                if receipt.worker_id.as_ref() == Some(worker_id)
                    && receipt.attempt_id.as_ref() == Some(&attempt.id) =>
            {
                match self.commit_dispatch_running(record.clone(), receipt).await {
                    LeaseResponse::Granted(grant) => {
                        self.forget_redrive(&record.lease_id);
                        StoredDispatchOutcome::Granted(grant)
                    }
                    LeaseResponse::Denied(error) => {
                        warn!(lease_id = %record.lease_id, %error, "re-driven dispatch could not commit");
                        StoredDispatchOutcome::Unresolved(error)
                    }
                }
            }
            Err(error) if error.is_refusal() => {
                self.forget_redrive(&record.lease_id);
                if let Err(cleanup) = self
                    .cancel_targeted(record, CancelIntent::Release, LeaseReleasedReason::Failed)
                    .await
                {
                    warn!(lease_id = %record.lease_id, %cleanup, "refused re-driven dispatch cleanup remains pending");
                }
                StoredDispatchOutcome::Refused(error.denial().cloned().unwrap_or_else(|| LeaseError::Backend {
                    message: error.to_string(),
                }))
            }
            Ok(_) => StoredDispatchOutcome::Unresolved(LeaseError::Backend {
                message: "provider receipt identity mismatch".into(),
            }),
            Err(error) => StoredDispatchOutcome::Unresolved(LeaseError::Backend {
                message: format!(
                    "dispatch outcome for lease {} remains pending: {error}",
                    record.lease_id.as_str()
                ),
            }),
        }
    }

    fn forget_redrive(&self, lease_id: &LeaseId) {
        self.pending_redrives().remove(lease_id.as_str());
    }
}
