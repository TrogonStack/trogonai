use super::*;

impl BrokerCore {
    pub(in crate::core) async fn dispatch_targeted(&self, admission: TargetedAdmission) -> LeaseResponse {
        let TargetedAdmission {
            entity_id,
            lease_id,
            provider,
            mut target,
            epoch_capable,
            request,
            mut dispatch,
            drive_records,
            correlation_id,
            deadline,
            operation,
        } = admission;
        let key = request.idempotency_key.clone();
        let epoch = epoch_for_assignment(None, &target.worker_id, &target.attempt_id);
        target.epoch = epoch_capable.then_some(epoch);
        let record = LeaseRecord {
            entity_id: entity_id.clone(),
            lease_id: lease_id.clone(),
            tenant: request.tenant.clone(),
            provider,
            status: LeaseStatus::Pending,
            created_at: SystemTime::now(),
            deadline,
            spec: request.spec.clone(),
            sandbox_id: String::new(),
            cpu_seconds_consumed: request.spec.cpu_seconds().get(),
            wall_seconds_consumed: request.ttl_seconds,
            correlation_id: Some(correlation_id.clone()),
            tags: request.tags.clone(),
            name: request.name.clone(),
            worker_id: Some(target.worker_id.clone()),
            dispatch_attempt: Some(LeaseDispatchAttempt {
                id: target.attempt_id.clone(),
                epoch: target.epoch,
                idempotency_key: key.clone(),
                accounting: if request.prewarm.is_prewarm() {
                    LeaseAccounting::Prewarm
                } else {
                    LeaseAccounting::Tenant
                },
                cleanup_reason: None,
                cleanup_started_at: None,
                pending_dispatch: None,
            }),
            quota_reservation: None,
            epoch: Some(epoch),
            last_activity_at: None,
            paused_remaining_seconds: None,
            freeze_in_flight: None,
            auto_paused_at: None,
            idle_pause_retry: None,
            dormancy: None,
            restored_from: dispatch.spec.image().snapshot_digest().cloned(),
        };
        let _admission = self.attempt_lock(&lease_id).lock_owned().await;
        match self.idempotency.begin_attempt(&record.tenant, &key, &record).await {
            Ok(true) => (),
            Ok(false) => {
                let _ = self.snapshot_pins.unpin(&lease_id).await;
                let _ = self.resolved_digests.clear(&lease_id).await;
                return match self.idempotency.get_attempt(&record.tenant, &key).await {
                    Ok(Some(existing)) => self.attempt_response(existing).await,
                    _ => LeaseResponse::Denied(LeaseError::Backend {
                        message: "request attempt changed while reserving it; retry".into(),
                    }),
                };
            }
            Err(error) => {
                return LeaseResponse::Denied(LeaseError::Backend {
                    message: format!("dispatch journal reservation failed: {error}"),
                });
            }
        }
        if let Err(error) = self.active_leases.put(&record).await {
            let cleanup = self.discard_unstarted(&record).await;
            return LeaseResponse::Denied(LeaseError::Backend {
                message: format!("pending lease persist failed: {error}; preparation cleanup: {cleanup:?}"),
            });
        }
        let reservation = match Self::attempt_reservation(&record) {
            Ok(reservation) => reservation,
            Err(error) => {
                return LeaseResponse::Denied(LeaseError::Backend {
                    message: error.to_string(),
                });
            }
        };
        if let Err(error) = self.quota.reserve_attempt(&reservation, &self.quota_limits()).await {
            return match self.discard_unstarted(&record).await {
                Ok(()) => LeaseResponse::Denied(error),
                Err(cleanup) => LeaseResponse::Denied(LeaseError::Backend {
                    message: format!(
                        "quota admission failed: {error:?}; lease {} retains pending cleanup: {cleanup}",
                        lease_id.as_str()
                    ),
                }),
            };
        }
        let preparation = async {
            self.commit_credential_policy(&record.tenant, &lease_id, &request.spec)
                .await
                .map_err(|denial| DispatchTransactionError::CredentialPolicy {
                    message: format!("{denial:?}"),
                })?;
            for mount in request.spec.drives() {
                self.ensure_preparing(&record).await?;
                self.drive_store
                    .set_attached_lease(&mount.drive_id, Some(lease_id.clone()))
                    .await?;
                if let Err(error) = self.ensure_preparing(&record).await {
                    self.drive_store.release_claim(&mount.drive_id, &lease_id).await?;
                    return Err(error);
                }
            }
            self.ensure_preparing(&record).await?;
            dispatch.spec = with_remote_drive_mounts(&dispatch.spec, &drive_records)?;
            let resolved_drives = drive_records
                .into_values()
                .map(|mut drive| {
                    drive.attached_lease = Some(lease_id.clone());
                    drive
                })
                .collect();
            let payload = trogon_sandbox_contract::ProviderDispatchPayload {
                lease_id: lease_id.clone(),
                request: dispatch,
                correlation_id: Some(correlation_id),
                resolved_drives,
                target: Some(target.clone()),
            };
            if let Some((operations, token)) = &operation
                && !operations.save_dispatch(token, &payload).await?
            {
                return Err(DispatchTransactionError::OperationClaimLost);
            }
            let payload = Bytes::from(serde_json::to_vec(&payload)?);
            Ok::<_, DispatchTransactionError>(payload)
        }
        .await;
        let payload = match preparation {
            Ok(payload) => payload,
            Err(error) => {
                let cleanup = self.discard_unstarted(&record).await;
                return LeaseResponse::Denied(LeaseError::Backend {
                    message: match cleanup {
                        Ok(()) => format!("dispatch preparation failed: {error}"),
                        Err(cleanup) => format!(
                            "dispatch preparation failed: {error}; lease {} retains pending cleanup: {cleanup}",
                            lease_id.as_str()
                        ),
                    },
                });
            }
        };
        let record = self.journal_pending_dispatch(record, &key, &payload).await;
        drop(_admission);
        let redrive_payload = payload.clone();
        let reply = self
            .request_dispatch_receipt(record.provider, &target.worker_id, payload)
            .await;
        let receipt = match reply {
            Ok(receipt)
                if receipt.worker_id.as_ref() == Some(&target.worker_id)
                    && receipt.attempt_id.as_ref() == Some(&target.attempt_id) =>
            {
                receipt
            }
            other => {
                if let Err(error) = &other
                    && let Some(denial) = error.denial()
                {
                    // A worker that explicitly refused the attempt is asked now to confirm its
                    // teardown; otherwise the refused lease holds its claims until its deadline.
                    // An undecodable reply stays pending. A failed cleanup leaves the lease
                    // Cancelling for the reconciler to retry.
                    if error.is_refusal()
                        && let Err(cleanup) = self
                            .cancel_targeted(&record, CancelIntent::Release, LeaseReleasedReason::Failed)
                            .await
                    {
                        warn!(lease_id = %lease_id, error = %denial, %cleanup, "provider denied dispatch; exact-attempt cleanup remains pending");
                    }
                    return LeaseResponse::Denied(denial.clone());
                }
                // The worker may have launched the attempt even though no usable reply
                // arrived. Workers dedupe a re-sent attempt id, so keep the payload for the
                // reconciler to re-send instead of waiting for the deadline.
                self.pending_redrives().insert(
                    lease_id.as_str().to_owned(),
                    PendingRedrive {
                        payload: redrive_payload,
                        last_sent: SystemTime::now(),
                    },
                );
                return LeaseResponse::Denied(LeaseError::Backend {
                    message: format!(
                        "dispatch outcome for lease {} remains pending: {}",
                        lease_id.as_str(),
                        match other {
                            Ok(_) => "provider receipt identity mismatch".into(),
                            Err(error) => error.to_string(),
                        }
                    ),
                });
            }
        };
        self.commit_dispatch_running(record, receipt).await
    }

    /// Moves a Pending targeted attempt to Running once the worker's receipt
    /// matched, and publishes the grant.
    pub(super) async fn commit_dispatch_running(
        &self,
        mut record: LeaseRecord,
        receipt: trogon_sandbox_contract::ProviderLease,
    ) -> LeaseResponse {
        let lease_id = record.lease_id.clone();
        let deadline = record.deadline;
        let key = match record.dispatch_attempt.as_ref() {
            Some(attempt) => attempt.idempotency_key.clone(),
            None => {
                return LeaseResponse::Denied(LeaseError::Backend {
                    message: format!("lease {} has no dispatch attempt", lease_id.as_str()),
                });
            }
        };
        let _commit = self.attempt_lock(&lease_id).lock_owned().await;
        let mut running = record.clone();
        running.status = LeaseStatus::Running;
        running.sandbox_id = receipt.sandbox_id.to_string();
        if let Some(attempt) = running.dispatch_attempt.as_mut() {
            attempt.pending_dispatch = None;
        }
        match self
            .idempotency
            .replace_attempt(&record.tenant, &key, &record, &running)
            .await
        {
            Ok(true) => (),
            _ => {
                return LeaseResponse::Denied(LeaseError::Backend {
                    message: format!(
                        "lease {} could not commit its grant; pending cleanup remains actionable",
                        lease_id.as_str()
                    ),
                });
            }
        }
        record = running;
        if let Err(error) = self.active_leases.put(&record).await {
            return LeaseResponse::Denied(LeaseError::Backend {
                message: format!("lease {} grant index failed: {error}", lease_id.as_str()),
            });
        }
        if let Some(name) = &record.name {
            let _ = self.named_leases.set(&record.tenant, name.as_str(), &lease_id).await;
        }
        let grant = LeaseGranted {
            entity_id: record.entity_id.clone(),
            operation_id: None,
            lease_id: lease_id.clone(),
            provider: record.provider,
            sandbox_id: record.sandbox_id.clone(),
            evidence_subject: subjects::evidence_subject(&lease_id),
            deadline,
            created: LeaseCreation::Created,
            capabilities: Vec::new(),
        };
        self.metrics
            .record_dispatch(record.tenant.as_str(), record.provider, "granted");
        self.metrics.inc_active_leases(record.tenant.as_str());
        LeaseResponse::Granted(grant)
    }

    /// Journals the dispatch about to be sent on the Pending attempt, so a
    /// broker that restarts before the reply commits can still re-send it.
    /// Best effort: if the journal write loses, the in-memory redrive still
    /// covers this process and the lease falls back to its deadline otherwise.
    pub(super) async fn journal_pending_dispatch(
        &self,
        record: LeaseRecord,
        key: &LeaseRequestKey,
        payload: &Bytes,
    ) -> LeaseRecord {
        let Ok(payload) = serde_json::from_slice(payload) else {
            return record;
        };
        let mut journaled = record.clone();
        let Some(attempt) = journaled.dispatch_attempt.as_mut() else {
            return record;
        };
        attempt.pending_dispatch = Some(trogon_sandbox_contract::PendingDispatch {
            sent_at: SystemTime::now(),
            payload,
        });
        match self
            .idempotency
            .replace_attempt(&record.tenant, key, &record, &journaled)
            .await
        {
            Ok(true) => {}
            Ok(false) => return record,
            Err(error) => {
                warn!(lease_id = %record.lease_id, %error, "pending dispatch journal write failed");
                return record;
            }
        }
        if let Err(error) = self.active_leases.put(&journaled).await {
            warn!(lease_id = %journaled.lease_id, %error, "pending dispatch index write failed");
        }
        journaled
    }
}
