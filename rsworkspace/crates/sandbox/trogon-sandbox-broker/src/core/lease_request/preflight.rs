use super::*;

impl BrokerCore {
    pub(super) async fn preflight_lease_request(
        &self,
        req: &mut LeaseRequest,
        ctx: &mut CreateCtx,
    ) -> Result<
        std::collections::HashMap<trogon_sandbox_contract::DriveId, trogon_sandbox_contract::DriveRecord>,
        LeaseResponse,
    > {
        if let Err(reason) = validate_spec(req, self.lifetime.max_lease_lifetime_seconds) {
            warn!(reason = %reason, "lease request denied: invalid spec");
            self.metrics
                .record_dispatch(req.tenant.as_str(), ProviderKind::Mock, "denied_invalid_spec");
            return Err(LeaseResponse::Denied(LeaseError::InvalidSpec {
                reason: reason.to_string(),
            }));
        }

        if let Err(e) = self.lifetime.check(req) {
            warn!(error = ?e, tenant = %req.tenant, "lease request denied: lifetime policy");
            self.metrics
                .record_dispatch(req.tenant.as_str(), ProviderKind::Mock, "denied_lifetime");
            return Err(LeaseResponse::Denied(e));
        }

        let request_key = req.idempotency_key.clone();
        match self.idempotency.get_attempt(&req.tenant, &request_key).await {
            Ok(Some(record)) => return Err(self.attempt_response(record).await),
            Ok(None) => (),
            Err(error) => {
                return Err(LeaseResponse::Denied(LeaseError::Backend {
                    message: format!("dispatch journal lookup failed: {error}"),
                }));
            }
        }

        let mut drive_records = std::collections::HashMap::new();
        for mount in req.spec.drives() {
            match self.drive_store.get(&mount.drive_id).await {
                Ok(Some(record))
                    if record.tenant != req.tenant || record.id != mount.drive_id || record.id.tenant != req.tenant =>
                {
                    warn!(
                        tenant = %req.tenant,
                        drive = %mount.drive_id.name,
                        "lease request denied: drive belongs to a different tenant"
                    );
                    self.metrics
                        .record_dispatch(req.tenant.as_str(), ProviderKind::Mock, "denied_drive_not_found");
                    return Err(LeaseResponse::Denied(LeaseError::InvalidSpec {
                        reason: DriveError::DriveNotFound {
                            drive_id: mount.drive_id.clone(),
                        }
                        .to_string(),
                    }));
                }
                Ok(Some(record)) => {
                    // A re-driven create finds the drive attached to its own lease.
                    if let Some(lease_id) = record.attached_lease.clone()
                        && ctx.preassigned.as_ref().is_none_or(|p| p.lease_id != lease_id)
                    {
                        warn!(
                            tenant = %req.tenant,
                            drive = %mount.drive_id.name,
                            lease_id = %lease_id,
                            "lease request denied: drive already attached to another lease"
                        );
                        self.metrics
                            .record_dispatch(req.tenant.as_str(), ProviderKind::Mock, "denied_drive_attached");
                        return Err(LeaseResponse::Denied(LeaseError::InvalidSpec {
                            reason: DriveError::DriveAttached {
                                drive_id: mount.drive_id.clone(),
                                lease_id,
                            }
                            .to_string(),
                        }));
                    }
                    drive_records.insert(mount.drive_id.clone(), record);
                }
                Ok(None) => {
                    warn!(
                        tenant = %req.tenant,
                        drive = %mount.drive_id.name,
                        "lease request denied: drive not found"
                    );
                    self.metrics
                        .record_dispatch(req.tenant.as_str(), ProviderKind::Mock, "denied_drive_not_found");
                    return Err(LeaseResponse::Denied(LeaseError::InvalidSpec {
                        reason: DriveError::DriveNotFound {
                            drive_id: mount.drive_id.clone(),
                        }
                        .to_string(),
                    }));
                }
                Err(e) => {
                    warn!(error = %e, "lease request denied: drive store lookup failed");
                    return Err(LeaseResponse::Denied(LeaseError::Backend { message: e.to_string() }));
                }
            }
        }

        if let Some(network_policy) = req.spec.network_policy()
            && let Err(reason) = validate_network_policy(network_policy)
        {
            warn!(reason = %reason, "lease request denied: invalid network policy");
            let reason = reason.to_string();
            self.metrics
                .record_dispatch(req.tenant.as_str(), ProviderKind::Mock, "denied_invalid_network_policy");
            return Err(LeaseResponse::Denied(LeaseError::InvalidNetworkPolicy { reason }));
        }

        // Named-lease reconnect: if `name` already maps to a still-Running
        // lease, hand that lease back instead of creating a new one. A stale
        // (not-found or not-Running) index entry falls through to normal
        // creation below, where it gets overwritten once the new lease is
        // granted; the snapshot-tag mechanism (not the active-lease store)
        // carries resumability across that gap.
        if let Some(name) = &req.name {
            match self.resolve_named_lease(&req.tenant, name.as_str()).await {
                Ok(Some(existing_lease_id)) => match self.active_leases.get(&existing_lease_id).await {
                    Ok(Some(record)) if record.tenant != req.tenant || record.lease_id != existing_lease_id => {
                        return Err(LeaseResponse::Denied(LeaseError::Backend {
                            message: "named lease index ownership mismatch".into(),
                        }));
                    }
                    Ok(Some(record))
                        if record.status == LeaseStatus::Running
                            && record.tenant == req.tenant
                            && record.lease_id == existing_lease_id
                            && !self.purge_pending(&existing_lease_id).await =>
                    {
                        info!(
                            tenant = %req.tenant,
                            name = %name,
                            lease_id = %existing_lease_id,
                            "named lease reconnect: returning existing running lease"
                        );
                        let mut granted = LeaseGranted {
                            entity_id: None,
                            operation_id: None,
                            lease_id: existing_lease_id,
                            provider: record.provider,
                            sandbox_id: record.sandbox_id.clone(),
                            evidence_subject: subjects::evidence_subject(&record.lease_id),
                            deadline: record.deadline,
                            created: LeaseCreation::Existing,
                            capabilities: Vec::new(),
                        };
                        if let Err(e) = self.entity_stamp_existing(req, &mut granted).await {
                            return Err(LeaseResponse::Denied(e));
                        }
                        return Err(LeaseResponse::Granted(granted));
                    }
                    Ok(_) => {}
                    Err(e) => {
                        warn!(error = %e, tenant = %req.tenant, name = %name, "lease request denied: active lease lookup failed during named reconnect check");
                        return Err(LeaseResponse::Denied(LeaseError::Backend { message: e.to_string() }));
                    }
                },
                Ok(None) => {}
                Err(e) => {
                    warn!(error = %e, tenant = %req.tenant, name = %name, "lease request denied: named lease lookup failed");
                    return Err(LeaseResponse::Denied(e));
                }
            }

            // A lease that turned snapshots off is never auto-captured, even
            // under a name; it still reconnects by name while it runs.
            if req.persistent.is_persistent()
                && req.spec.retention().snapshots == SnapshotCaptureAccess::Allowed
                && matches!(req.spec.image(), SandboxImage::Registry { .. })
                && req.spec.snapshot_out().is_none()
            {
                let original_image = req.spec.image().clone();
                let artifact_type = if req.provider_pref.contains(&ProviderKind::Firecracker) {
                    if req
                        .provider_pref
                        .iter()
                        .any(|provider| *provider != ProviderKind::Firecracker)
                    {
                        return Err(LeaseResponse::Denied(LeaseError::InvalidSpec { reason: "automatic named persistence with Firecracker requires a Firecracker-only provider preference or an explicit snapshot_out artifact type".into() }));
                    }
                    ArtifactType::FirecrackerSnapshotV1
                } else {
                    ArtifactType::SandboxRootfsV1
                };
                if crate::worker_registry::lacks_snapshot_persistence(
                    self.worker_registry.as_ref(),
                    &req.provider_pref,
                    artifact_type,
                )
                .await
                {
                    // Every preferred provider is live and none can restore or
                    // export a snapshot, so a SnapshotTag image would be
                    // rejected by the worker. Keep the lease named but
                    // uncaptured, like `retention.snapshots = false`.
                    info!(
                        tenant = %req.tenant,
                        name = %name,
                        "named lease: preferred providers lack snapshot support, skipping automatic persistence"
                    );
                } else {
                    let tag_name = match SnapshotTagName::new(name.as_str()) {
                        Ok(tag_name) => tag_name,
                        Err(e) => {
                            warn!(error = %e, tenant = %req.tenant, name = %name, "lease request denied: invalid snapshot tag name");
                            return Err(LeaseResponse::Denied(LeaseError::InvalidSpec { reason: e.to_string() }));
                        }
                    };
                    let options = PersistentSnapshotOptions {
                        tag_name,
                        artifact_type,
                        target_repository: snapshot_repository_for_tenant(&req.tenant),
                        retention: None,
                        prewarm: None,
                    };
                    match with_persistence(req.spec.clone(), req.tenant.clone(), Some(original_image), options) {
                        Ok(spec) => req.spec = spec,
                        Err(e) => {
                            warn!(error = %e, tenant = %req.tenant, name = %name, "lease request denied: invalid snapshot tag name");
                            return Err(LeaseResponse::Denied(LeaseError::InvalidSpec { reason: e.to_string() }));
                        }
                    }
                }
            }
        }

        // Idempotency check: short-circuit on cache hit
        match self.idempotency.get(&req.tenant, req.idempotency_key.as_str()).await {
            Ok(Some(cached)) => {
                if crate::worker_registry::requires_targeted_dispatch(cached.provider) {
                    return Err(LeaseResponse::Denied(LeaseError::Backend {
                        message: "modern cached grant has no exact request journal; inspect its lease before retrying"
                            .into(),
                    }));
                }
                info!(
                    tenant = %req.tenant,
                    idempotency_key = %req.idempotency_key,
                    lease_id = %cached.lease_id,
                    "idempotent lease request: returning cached grant"
                );
                return Err(LeaseResponse::Granted(LeaseGranted {
                    created: LeaseCreation::Existing,
                    ..cached
                }));
            }
            Ok(None) => {}
            Err(e) => {
                warn!(error = %e, "idempotency store read error; proceeding without cache");
            }
        }

        Ok(drive_records)
    }
}
