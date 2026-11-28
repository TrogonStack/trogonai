use super::*;

mod legacy_dispatch;
mod policy;
mod preflight;
mod selection;

#[derive(Clone, Copy)]
struct InitialLeaseQuota {
    reserved: bool,
    cpu_seconds: u32,
    wall_seconds: u32,
}

impl BrokerCore {
    #[tracing::instrument(skip_all, fields(tenant = %req.tenant, idempotency_key = %req.idempotency_key, correlation_id))]
    pub async fn handle_lease_request(&self, req: LeaseRequest) -> LeaseResponse {
        if req.operation_id.is_some() && self.operations.is_none() {
            return LeaseResponse::Denied(LeaseError::InvalidSpec { reason: "durable operations are not enabled; the requested operation cannot fall back to an untracked create".into() });
        }
        if req.operation_id.is_some() && self.operations.is_some() {
            let response = self.create_operation(req).await;
            return self.with_provider_capabilities(response).await;
        }
        let tenant = req.tenant.clone();
        let mut ctx = CreateCtx::default();
        let response = self.lease_request_inner(req, &mut ctx).await;
        self.entity_finish_start(
            &tenant,
            ctx.name_claim.as_ref().map(|(_, name, _)| name.as_str()),
            ctx.name_claim.as_ref().map(|(_, _, lease)| lease),
            &response,
        )
        .await;
        // A name claimed before provisioning must not outlive a failed grant.
        if matches!(response, LeaseResponse::Denied(_))
            && let Some((tenant, name, lease_id)) = ctx.name_claim
            && let Err(e) = self.named_leases.clear_if_matches(&tenant, &name, &lease_id).await
        {
            warn!(error = %e, tenant = %tenant, name = %name, lease_id = %lease_id, "failed to release named lease claim after denied request");
        }
        self.with_provider_capabilities(response).await
    }

    async fn with_provider_capabilities(&self, response: LeaseResponse) -> LeaseResponse {
        match response {
            LeaseResponse::Granted(mut granted) => {
                granted.capabilities.clear();
                let selected = match self.active_leases.get(&granted.lease_id).await {
                    Ok(Some(record))
                        if record.lease_id == granted.lease_id
                            && record.provider == granted.provider
                            && record.sandbox_id == granted.sandbox_id
                            && record.status == LeaseStatus::Running =>
                    {
                        match record.worker_id.as_ref() {
                            Some(worker) => self
                                .worker_registry
                                .current_worker(record.provider, worker)
                                .await
                                .filter(|heartbeat| {
                                    heartbeat.worker_id == *worker
                                        && heartbeat.provider == record.provider
                                        && heartbeat.profile.provider == record.provider
                                })
                                .map(|heartbeat| (record.clone(), heartbeat.profile)),
                            None if record.dispatch_attempt.is_none() && record.epoch.is_none() => self
                                .worker_registry
                                .profile_for_provider(record.provider)
                                .await
                                .filter(|profile| profile.provider == record.provider)
                                .map(|profile| (record.clone(), profile)),
                            None => None,
                        }
                    }
                    _ => None,
                };
                if let Some((record, profile)) = selected {
                    granted.capabilities = profile
                        .sdk_features()
                        .into_iter()
                        .map(|feature| feature.as_str().to_owned())
                        .collect();
                    if (profile.isolation.supports_command_cancellation.is_supported()
                        || profile.isolation.supports_command_launch_cancellation.is_supported())
                        && !self.command_readiness_for_grant(&granted, &record, &profile).await
                    {
                        granted.capabilities.retain(|feature| {
                            feature != "command_cancellation" && feature != "command_launch_cancellation"
                        });
                    }
                }
                if self.command_logs.is_some() {
                    granted.capabilities.push("read_logs".to_owned());
                }
                LeaseResponse::Granted(granted)
            }
            other => other,
        }
    }

    /// Atomically claims `name` for `lease_id`, or resolves the existing
    /// holder: a running lease is returned (get-or-create), a stale mapping is
    /// taken over with a compare-and-swap, and a mapping too young to be stale
    /// whose lease has no record yet is another request still provisioning.
    async fn claim_lease_name(
        &self,
        tenant: &TenantId,
        name: &trogon_sandbox_contract::LeaseName,
        lease_id: &LeaseId,
    ) -> Result<NameClaimOutcome, LeaseError> {
        let backend = |message: String| LeaseError::Backend { message };
        for _ in 0..4 {
            let pending_ttl = self.config.provider_dispatch_timeout;
            let (holder, pending) = match self
                .named_leases
                .claim(tenant, name.as_str(), lease_id, pending_ttl)
                .await
                .map_err(|e| backend(e.to_string()))?
            {
                NameClaim::Claimed => return self.resolve_own_name_claim(tenant, lease_id).await,
                NameClaim::Held { lease, pending, .. } => (lease, pending),
            };
            match self
                .active_leases
                .get(&holder)
                .await
                .map_err(|e| backend(e.to_string()))?
            {
                Some(record) if record.tenant != *tenant || record.lease_id != holder => {
                    return Err(backend("named lease index ownership mismatch".into()));
                }
                Some(record) if record.status == LeaseStatus::Running && !self.purge_pending(&holder).await => {
                    return Ok(NameClaimOutcome::Existing(LeaseGranted {
                        entity_id: None,
                        operation_id: None,
                        lease_id: holder,
                        provider: record.provider,
                        sandbox_id: record.sandbox_id.clone(),
                        evidence_subject: subjects::evidence_subject(&record.lease_id),
                        deadline: record.deadline,
                        created: LeaseCreation::Existing,
                        capabilities: Vec::new(),
                    }));
                }
                None if pending => {
                    return Err(LeaseError::NameConflict { name: name.clone() });
                }
                _ => {}
            }
            if self
                .named_leases
                .replace_if_matches(tenant, name.as_str(), &holder, lease_id, pending_ttl)
                .await
                .map_err(|e| backend(e.to_string()))?
            {
                return self.resolve_own_name_claim(tenant, lease_id).await;
            }
        }
        Err(backend("named lease claim exhausted retries".into()))
    }

    /// A claim won under a lease id derived from the idempotency key can be a
    /// retry of a request that already provisioned that lease; hand back the
    /// running lease instead of provisioning a second sandbox under it.
    async fn resolve_own_name_claim(
        &self,
        tenant: &TenantId,
        lease_id: &LeaseId,
    ) -> Result<NameClaimOutcome, LeaseError> {
        let existing = self
            .active_leases
            .get(lease_id)
            .await
            .map_err(|e| LeaseError::Backend { message: e.to_string() })?;
        Ok(match existing {
            Some(record)
                if record.tenant == *tenant
                    && record.lease_id == *lease_id
                    && record.status == LeaseStatus::Running =>
            {
                NameClaimOutcome::Existing(LeaseGranted {
                    entity_id: None,
                    operation_id: None,
                    lease_id: lease_id.clone(),
                    provider: record.provider,
                    sandbox_id: record.sandbox_id.clone(),
                    evidence_subject: subjects::evidence_subject(&record.lease_id),
                    deadline: record.deadline,
                    created: LeaseCreation::Existing,
                    capabilities: Vec::new(),
                })
            }
            _ => NameClaimOutcome::Claimed,
        })
    }

    #[cfg_attr(
        dylint_lib = "trogon_dylint_lints",
        expect(telemetry_attribute_literal, reason = "the attribute key is a local literal")
    )]
    pub(super) async fn lease_request_inner(&self, mut req: LeaseRequest, ctx: &mut CreateCtx) -> LeaseResponse {
        let correlation_id = generated_correlation_id();
        tracing::Span::current().record("correlation_id", correlation_id.as_str());

        let drive_records = match self.preflight_lease_request(&mut req, ctx).await {
            Ok(records) => records,
            Err(response) => return response,
        };

        let cpu_seconds = req.spec.cpu_seconds().get();
        // The wall seconds this lease is charged and later refunded. A no-TTL
        // lease has no deadline to price, so it is charged the operator's
        // lifetime cap: the most any capped lease could hold.
        let ttl_seconds = ctx.quota_reservation.as_ref().map_or_else(
            || {
                if req.spec.no_ttl().is_unbounded() {
                    self.lifetime.max_lease_lifetime_seconds
                } else {
                    req.ttl_seconds
                }
            },
            |reservation| reservation.wall_seconds,
        );
        let tenant = req.tenant.clone();
        let prewarm = req.prewarm;
        let potentially_targeted = req.provider_pref.iter().copied().any(|provider| {
            crate::worker_registry::requires_targeted_dispatch(provider)
                || crate::worker_registry::prefers_targeted_dispatch(provider)
        });
        let mut initial_quota_reserved = false;

        let native_snapshot_provider = match self.check_lease_policy(&req, ctx).await {
            Ok(provider) => provider,
            Err(response) => return response,
        };
        if !potentially_targeted && (!prewarm.is_prewarm() || ctx.operation.is_some()) {
            if let Err(error) = self
                .reserve_initial_quota(ctx, &tenant, cpu_seconds, ttl_seconds, prewarm)
                .await
            {
                return LeaseResponse::Denied(error);
            }
            initial_quota_reserved = true;
        }

        let selection::SelectedLease {
            mut dispatch_req,
            tag_resolved_digest,
            selection,
        } = match self
            .select_lease_worker(
                &req,
                ctx,
                &drive_records,
                native_snapshot_provider,
                InitialLeaseQuota {
                    reserved: initial_quota_reserved,
                    cpu_seconds,
                    wall_seconds: ttl_seconds,
                },
            )
            .await
        {
            Ok(selected) => selected,
            Err(response) => return response,
        };
        let snapshot_digest = tag_resolved_digest
            .as_ref()
            .or_else(|| dispatch_req.spec.image().snapshot_digest());

        let kind = selection.provider;
        // The worker of the chosen provider gets only its own native options.
        dispatch_req.native = req.native.as_ref().and_then(|native| native_for_provider(native, kind));
        if selection.worker_id.is_none()
            && (!prewarm.is_prewarm() || ctx.operation.is_some())
            && !initial_quota_reserved
        {
            if let Err(error) = self
                .reserve_initial_quota(ctx, &tenant, cpu_seconds, ttl_seconds, prewarm)
                .await
            {
                return LeaseResponse::Denied(error);
            }
            initial_quota_reserved = true;
        }
        if selection.locality_hit {
            self.metrics
                .locality_hit_total
                .with_label_values(&[trogon_sandbox_protocol::subjects::provider_token(kind)])
                .inc();
        }

        if matches!(
            kind,
            trogon_sandbox_contract::ProviderKind::K8sMicroVm
                | trogon_sandbox_contract::ProviderKind::Daytona
                | trogon_sandbox_contract::ProviderKind::Modal
                | trogon_sandbox_contract::ProviderKind::Hyperlight
                | trogon_sandbox_contract::ProviderKind::Substrate
                | trogon_sandbox_contract::ProviderKind::Firecracker
                | trogon_sandbox_contract::ProviderKind::Fly
                | trogon_sandbox_contract::ProviderKind::Proxmox
                | trogon_sandbox_contract::ProviderKind::E2b
                | trogon_sandbox_contract::ProviderKind::Vercel
                | trogon_sandbox_contract::ProviderKind::Space
        ) && let WorkspaceSource::HostMount { .. } = &req.spec.workspace()
        {
            self.refund_initial_quota(initial_quota_reserved, &tenant, cpu_seconds, ttl_seconds, ctx)
                .await;
            warn!(tenant = %tenant, "lease request denied: provider cannot access host_mount workspaces");
            return LeaseResponse::Denied(LeaseError::InvalidSpec {
                reason: "host_mount workspaces are not supported by this provider".to_string(),
            });
        }

        if kind == trogon_sandbox_contract::ProviderKind::Substrate
            && let Some(reason) = substrate_unsupported_spec_reason(&req.spec)
        {
            self.refund_initial_quota(initial_quota_reserved, &tenant, cpu_seconds, ttl_seconds, ctx)
                .await;
            warn!(tenant = %tenant, reason = %reason, "lease request denied: spec not supported on substrate");
            return LeaseResponse::Denied(LeaseError::InvalidSpec { reason });
        }

        if let Err(e) = self.host_import_policy.validate(&tenant, &req.host_imports).await {
            self.refund_initial_quota(initial_quota_reserved, &tenant, cpu_seconds, ttl_seconds, ctx)
                .await;
            warn!(tenant = %tenant, "lease request denied: host_import policy rejected");
            return LeaseResponse::Denied(e);
        }

        if let Err(e) = self
            .hardening_override_policy
            .validate(&tenant, &req.spec.platform().container)
        {
            self.refund_initial_quota(initial_quota_reserved, &tenant, cpu_seconds, ttl_seconds, ctx)
                .await;
            warn!(tenant = %tenant, "lease request denied: hardening override policy rejected");
            return LeaseResponse::Denied(e);
        }

        let lease_id = match (&ctx.preassigned, &req.name) {
            (Some(chosen), _) => chosen.lease_id.clone(),
            (None, Some(name)) => derived_named_lease_id(&tenant, name.as_str(), req.idempotency_key.as_str()),
            (None, None) => generated_lease_id(),
        };
        let deadline = if req.spec.no_ttl().is_unbounded() {
            SystemTime::now() + LifetimePolicy::NO_TTL_DEADLINE_OFFSET
        } else {
            SystemTime::now() + Duration::from_secs(u64::from(ttl_seconds))
        };

        let mut entity_id: Option<trogon_sandbox_contract::SandboxEntityId> = None;
        if let Some(name) = &req.name {
            match self.claim_lease_name(&tenant, name, &lease_id).await {
                Ok(NameClaimOutcome::Claimed) => {
                    ctx.name_claim = Some((tenant.clone(), name.to_string(), lease_id.clone()));
                    match self
                        .entity_begin_start(&req, &lease_id, tag_resolved_digest.as_ref())
                        .await
                    {
                        Ok(id) => entity_id = id,
                        Err(e) => {
                            self.refund_initial_quota(initial_quota_reserved, &tenant, cpu_seconds, ttl_seconds, ctx)
                                .await;
                            return LeaseResponse::Denied(e);
                        }
                    }
                }
                Ok(NameClaimOutcome::Existing(mut granted)) => {
                    let stamped = self.entity_stamp_existing(&req, &mut granted).await;
                    self.refund_initial_quota(initial_quota_reserved, &tenant, cpu_seconds, ttl_seconds, ctx)
                        .await;
                    if let Err(e) = stamped {
                        return LeaseResponse::Denied(e);
                    }
                    info!(
                        tenant = %tenant,
                        name = %name,
                        lease_id = %granted.lease_id,
                        "named lease claim lost: returning existing running lease"
                    );
                    return LeaseResponse::Granted(granted);
                }
                Err(e) => {
                    self.refund_initial_quota(initial_quota_reserved, &tenant, cpu_seconds, ttl_seconds, ctx)
                        .await;
                    warn!(tenant = %tenant, name = %name, "lease request denied: named lease claim failed");
                    return LeaseResponse::Denied(e);
                }
            }
        }

        if let Some(digest) = &tag_resolved_digest
            && let Err(e) = self.resolved_digests.record(&lease_id, digest).await
        {
            self.refund_initial_quota(initial_quota_reserved, &tenant, cpu_seconds, ttl_seconds, ctx)
                .await;
            warn!(error = %e, "lease request denied: resolved digest record failed");
            return LeaseResponse::Denied(LeaseError::Backend { message: e.to_string() });
        }

        if let Some(digest) = snapshot_digest
            && let Err(e) = self.snapshot_pins.pin(&lease_id, digest).await
        {
            self.refund_initial_quota(initial_quota_reserved, &tenant, cpu_seconds, ttl_seconds, ctx)
                .await;
            warn!(error = %e, "lease request denied: snapshot digest pin failed");
            return LeaseResponse::Denied(LeaseError::Backend { message: e.to_string() });
        }

        if let Some(digest) = snapshot_digest
            && let Err(e) = self.snapshot_last_used.touch(digest, SystemTime::now()).await
        {
            warn!(
                error = %e,
                digest = %digest,
                "failed to record snapshot last-used timestamp; retention age reset-on-use degraded for this lease"
            );
        }

        if let Some(worker_id) = selection.worker_id {
            let epoch_capable = self
                .worker_registry
                .eligible_for(kind, None)
                .await
                .iter()
                .any(|worker| worker.supports_lease_epoch.is_supported() && worker.worker_id == worker_id);
            let target = trogon_sandbox_contract::DispatchTarget {
                worker_id,
                attempt_id: match &ctx.preassigned {
                    Some(chosen) => chosen.attempt_id.clone(),
                    None => match generated_attempt_id() {
                        Ok(attempt_id) => attempt_id,
                        Err(error) => return LeaseResponse::Denied(backend(error.to_string())),
                    },
                },
                epoch: None,
            };
            return self
                .dispatch_targeted(dispatch_transaction::TargetedAdmission {
                    entity_id,
                    lease_id,
                    provider: kind,
                    target,
                    request: req,
                    epoch_capable,
                    dispatch: dispatch_req,
                    drive_records,
                    correlation_id,
                    deadline,
                    operation: ctx.operation.clone(),
                })
                .await;
        }

        let restored_from = snapshot_digest.cloned();
        self.dispatch_legacy_lease(
            legacy_dispatch::LegacyLeaseAdmission {
                req,
                dispatch_req,
                drive_records,
                correlation_id,
                lease_id,
                kind,
                deadline,
                entity_id,
                snapshot_digest: restored_from,
                quota: InitialLeaseQuota {
                    reserved: initial_quota_reserved,
                    cpu_seconds,
                    wall_seconds: ttl_seconds,
                },
            },
            ctx,
        )
        .await
    }
}
