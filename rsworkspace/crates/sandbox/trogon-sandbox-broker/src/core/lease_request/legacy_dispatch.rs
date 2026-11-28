use super::*;

pub(super) struct LegacyLeaseAdmission {
    pub(super) req: LeaseRequest,
    pub(super) dispatch_req: LeaseRequest,
    pub(super) drive_records:
        std::collections::HashMap<trogon_sandbox_contract::DriveId, trogon_sandbox_contract::DriveRecord>,
    pub(super) correlation_id: CorrelationId,
    pub(super) lease_id: LeaseId,
    pub(super) kind: ProviderKind,
    pub(super) deadline: SystemTime,
    pub(super) entity_id: Option<trogon_sandbox_contract::SandboxEntityId>,
    pub(super) snapshot_digest: Option<Digest>,
    pub(super) quota: InitialLeaseQuota,
}

impl BrokerCore {
    pub(super) async fn dispatch_legacy_lease(
        &self,
        admission: LegacyLeaseAdmission,
        ctx: &mut CreateCtx,
    ) -> LeaseResponse {
        let LegacyLeaseAdmission {
            req,
            mut dispatch_req,
            drive_records,
            correlation_id,
            lease_id,
            kind,
            deadline,
            entity_id,
            snapshot_digest,
            quota,
        } = admission;
        let InitialLeaseQuota {
            reserved: initial_quota_reserved,
            cpu_seconds,
            wall_seconds: ttl_seconds,
        } = quota;
        let tenant = req.tenant.clone();
        let is_prewarm = req.prewarm.is_prewarm();
        let snapshot_digest = snapshot_digest.as_ref();
        let mut claimed_drives: Vec<&trogon_sandbox_contract::DriveMount> = Vec::new();
        for mount in req.spec.drives() {
            match self
                .drive_store
                .set_attached_lease(&mount.drive_id, Some(lease_id.clone()))
                .await
            {
                Ok(()) => claimed_drives.push(mount),
                Err(e) => {
                    for claimed in &claimed_drives {
                        let _ = self.drive_store.release_claim(&claimed.drive_id, &lease_id).await;
                    }
                    self.refund_initial_quota(initial_quota_reserved, &tenant, cpu_seconds, ttl_seconds, ctx)
                        .await;
                    warn!(error = %e, drive = %mount.drive_id.name, "lease request denied: drive claim failed");
                    self.metrics
                        .record_dispatch(tenant.as_str(), ProviderKind::Mock, "denied_drive_attached");
                    let reason = if matches!(e, DriveStoreError::AlreadyAttached) {
                        format!("drive '{}' is already attached to another lease", mount.drive_id.name)
                    } else {
                        format!("drive claim failed: {e}")
                    };
                    return LeaseResponse::Denied(LeaseError::InvalidSpec { reason });
                }
            }
        }

        dispatch_req.spec = match with_remote_drive_mounts(&dispatch_req.spec, &drive_records) {
            Ok(spec) => spec,
            Err(error) => return LeaseResponse::Denied(error),
        };

        let resolved_drives = drive_records
            .values()
            .cloned()
            .map(|mut record| {
                record.attached_lease = Some(lease_id.clone());
                record
            })
            .collect::<Vec<_>>();
        let dispatch_payload = match build_dispatch_payload(&dispatch_req, &lease_id, &correlation_id, &resolved_drives)
        {
            Ok(p) => p,
            Err(e) => {
                self.refund_initial_quota(initial_quota_reserved, &tenant, cpu_seconds, ttl_seconds, ctx)
                    .await;
                warn!(error = %e, "lease request denied: dispatch payload build failed");
                self.metrics
                    .record_dispatch(tenant.as_str(), kind, "denied_dispatch_payload");
                return LeaseResponse::Denied(LeaseError::Backend { message: e.to_string() });
            }
        };

        if let Err(e) = self.commit_credential_policy(&tenant, &lease_id, &req.spec).await {
            self.refund_initial_quota(initial_quota_reserved, &tenant, cpu_seconds, ttl_seconds, ctx)
                .await;
            warn!(error = ?e, "lease request denied: credential policy commit failed");
            return LeaseResponse::Denied(e);
        }

        if initial_quota_reserved && let Err(error) = self.note_quota_reserved(ctx, cpu_seconds, ttl_seconds).await {
            return LeaseResponse::Denied(error);
        }

        if let Some((operations, token)) = &ctx.operation {
            let payload =
                match serde_json::from_slice::<trogon_sandbox_contract::ProviderDispatchPayload>(&dispatch_payload) {
                    Ok(payload) => payload,
                    Err(error) => {
                        self.refund_initial_quota(initial_quota_reserved, &tenant, cpu_seconds, ttl_seconds, ctx)
                            .await;
                        return LeaseResponse::Denied(LeaseError::Backend {
                            message: error.to_string(),
                        });
                    }
                };
            match operations.save_dispatch(token, &payload).await {
                Ok(true) => {}
                Ok(false) => {
                    ctx.quota_retry = true;
                    return LeaseResponse::Denied(LeaseError::Backend {
                        message: "operation claim was lost before the legacy dispatch intent was stored".into(),
                    });
                }
                Err(error) => {
                    ctx.quota_retry = true;
                    return LeaseResponse::Denied(LeaseError::Backend {
                        message: error.to_string(),
                    });
                }
            }
        }

        let subject = subjects::provider_dispatch(kind);
        let signed_dispatch = match self.sign_dispatch(dispatch_payload) {
            Ok(signed) => signed,
            Err(error) => {
                // Nothing was sent, so no worker can have launched.
                self.refund_initial_quota(initial_quota_reserved, &tenant, cpu_seconds, ttl_seconds, ctx)
                    .await;
                warn!(%error, provider = ?kind, "lease request denied: dispatch could not be signed");
                return LeaseResponse::Denied(LeaseError::Backend {
                    message: format!("dispatch could not be signed: {error}"),
                });
            }
        };
        let provider_reply = match self
            .bus
            .request_with_timeout(&subject, signed_dispatch, self.config.provider_dispatch_timeout)
            .await
        {
            Ok(r) => r,
            Err(e) => {
                // No usable reply: the worker may have launched anyway.
                ctx.dispatch_unknown = true;
                self.refund_initial_quota(initial_quota_reserved, &tenant, cpu_seconds, ttl_seconds, ctx)
                    .await;
                warn!(error = %e, provider = ?kind, "lease request denied: provider dispatch failed");
                return LeaseResponse::Denied(LeaseError::Backend { message: e.to_string() });
            }
        };

        let provider_lease = match decode_dispatch_reply(kind, &provider_reply) {
            Ok(l) => l,
            Err(e) => {
                if matches!(e, LeaseError::Backend { .. }) && ctx.operation.is_some() {
                    ctx.dispatch_unknown = true;
                }
                self.refund_initial_quota(initial_quota_reserved, &tenant, cpu_seconds, ttl_seconds, ctx)
                    .await;
                self.revoke_credential_policy(&tenant, &lease_id).await;
                warn!(error = %e, provider = ?kind, "lease request denied: provider did not grant");
                self.metrics
                    .record_dispatch(tenant.as_str(), kind, "denied_provider_failed");
                return LeaseResponse::Denied(e);
            }
        };

        let evidence_subject = subjects::evidence_subject(&lease_id);

        let record = LeaseRecord {
            epoch: None,
            entity_id: entity_id.clone(),
            lease_id: lease_id.clone(),
            tenant: req.tenant.clone(),
            provider: kind,
            status: LeaseStatus::Running,
            created_at: SystemTime::now(),
            deadline,
            spec: req.spec.clone(),
            cpu_seconds_consumed: cpu_seconds,
            wall_seconds_consumed: ttl_seconds,
            correlation_id: Some(correlation_id.clone()),
            tags: req.tags.clone(),
            sandbox_id: provider_lease.sandbox_id.to_string(),
            name: req.name.clone(),
            worker_id: provider_lease.worker_id.clone(),
            dispatch_attempt: None,
            quota_reservation: ctx.quota_reservation.clone(),
            last_activity_at: None,
            paused_remaining_seconds: None,
            freeze_in_flight: None,
            auto_paused_at: None,
            idle_pause_retry: None,
            dormancy: None,
            restored_from: snapshot_digest.cloned(),
        };

        if initial_quota_reserved && let Err(error) = self.note_quota_reserved(ctx, cpu_seconds, ttl_seconds).await {
            ctx.dispatch_unknown = true;
            return LeaseResponse::Denied(error);
        }

        // Persist active-lease record BEFORE replying. On failure, refund quota.
        if let Err(e) = self.active_leases.put(&record).await {
            ctx.dispatch_unknown = ctx.operation.is_some();
            self.refund_initial_quota(initial_quota_reserved, &tenant, cpu_seconds, ttl_seconds, ctx)
                .await;
            warn!(error = %e, "lease request denied: active lease record persist failed");
            return LeaseResponse::Denied(LeaseError::Backend { message: e.to_string() });
        }

        if !is_prewarm
            && ctx.quota_reservation.is_none()
            && let Err(e) = self.quota.increment_active_leases(&tenant).await
        {
            let _ = self.active_leases.delete(&lease_id).await;
            self.refund_initial_quota(initial_quota_reserved, &tenant, cpu_seconds, ttl_seconds, ctx)
                .await;
            if snapshot_digest.is_some() {
                let _ = self.snapshot_pins.unpin(&lease_id).await;
            }
            warn!(error = %e, "lease request denied: active lease increment failed");
            return LeaseResponse::Denied(LeaseError::Backend { message: e.to_string() });
        }
        if initial_quota_reserved {
            self.note_quota_counted(ctx).await;
        }

        if let Some(name) = &req.name
            && let Err(e) = self.named_leases.confirm(&tenant, name.as_str(), &lease_id).await
        {
            // The pending marker simply expires; the active record already
            // resolves the name to this lease.
            warn!(error = %e, tenant = %tenant, name = %name, "failed to confirm named lease claim");
        }

        if is_prewarm && ctx.quota_reservation.is_none() {
            let _ = self.quota.record_prewarm_dispatch(&tenant, cpu_seconds).await;
        }

        let granted = LeaseGranted {
            entity_id,
            operation_id: None,
            lease_id,
            provider: kind,
            sandbox_id: provider_lease.sandbox_id.to_string(),
            evidence_subject,
            deadline,
            created: LeaseCreation::Created,
            capabilities: Vec::new(),
        };

        // Cache the grant for idempotency. Create-only, so a concurrent request
        // with the same key that won the race keeps its lease and this one
        // gives its own back instead of leaving the tenant charged for two.
        let earlier_grant = match self
            .idempotency
            .put_if_absent(&req.tenant, req.idempotency_key.as_str(), &granted)
            .await
        {
            Ok(Some(earlier)) if earlier.lease_id != granted.lease_id => Some(earlier),
            Ok(_) => None,
            Err(e) => {
                warn!(error = %e, "failed to cache idempotency grant; duplicate may re-charge");
                None
            }
        };

        info!(
            lease_id = %granted.lease_id,
            tenant = %tenant,
            provider = ?kind,
            sandbox_id = %granted.sandbox_id,
            correlation_id = %correlation_id,
            "lease granted"
        );

        debug!(
            lease_id = %granted.lease_id,
            env_keys = ?req.spec.env().keys().collect::<Vec<_>>(),
            "lease granted env keys"
        );

        self.metrics.record_dispatch(tenant.as_str(), kind, "granted");
        self.metrics.inc_active_leases(tenant.as_str());

        if let Some(earlier) = earlier_grant {
            info!(
                lease_id = %granted.lease_id,
                kept_lease_id = %earlier.lease_id,
                tenant = %tenant,
                idempotency_key = %req.idempotency_key,
                "idempotent lease request lost the race; releasing the duplicate lease"
            );
            if let Err(e) = self
                .release_lease(
                    granted.lease_id.clone(),
                    CancelIntent::Purge,
                    LeaseReleasedReason::Cancelled,
                )
                .await
            {
                warn!(error = %e, lease_id = %granted.lease_id, "failed to release duplicate idempotent lease; the sweeper reclaims it at its deadline");
            }
            return LeaseResponse::Granted(LeaseGranted {
                created: LeaseCreation::Existing,
                ..earlier
            });
        }

        LeaseResponse::Granted(granted)
    }
}
