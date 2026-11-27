use super::*;

pub(super) struct SelectedLease {
    pub(super) dispatch_req: LeaseRequest,
    pub(super) tag_resolved_digest: Option<Digest>,
    pub(super) selection: ProviderSelection,
}

impl BrokerCore {
    pub(super) async fn select_lease_worker(
        &self,
        req: &LeaseRequest,
        ctx: &mut CreateCtx,
        drive_records: &std::collections::HashMap<
            trogon_sandbox_contract::DriveId,
            trogon_sandbox_contract::DriveRecord,
        >,
        native_snapshot_provider: Option<ProviderKind>,
        quota: InitialLeaseQuota,
    ) -> Result<SelectedLease, LeaseResponse> {
        let tenant = req.tenant.clone();
        let InitialLeaseQuota {
            reserved: initial_quota_reserved,
            cpu_seconds,
            wall_seconds: ttl_seconds,
        } = quota;
        if req.provider_pref.is_empty() {
            self.refund_initial_quota(initial_quota_reserved, &tenant, cpu_seconds, ttl_seconds, ctx)
                .await;
            warn!(tenant = %tenant, "lease request denied: no provider available");
            return Err(LeaseResponse::Denied(LeaseError::NoProviderAvailable));
        }

        let mut provider_pref: Vec<trogon_sandbox_contract::ProviderKind> = match req.isolation_tier {
            Some(tier) => {
                let allowed = tier.providers_supporting();
                let filtered: Vec<_> = req
                    .provider_pref
                    .iter()
                    .copied()
                    .filter(|k| allowed.contains(k))
                    .collect();
                if filtered.is_empty() {
                    self.refund_initial_quota(initial_quota_reserved, &tenant, cpu_seconds, ttl_seconds, ctx)
                        .await;
                    warn!(tenant = %tenant, ?tier, "lease request denied: no preferred provider supports isolation tier");
                    return Err(LeaseResponse::Denied(LeaseError::NoProviderAvailable));
                }
                filtered
            }
            None => req.provider_pref.clone(),
        };
        provider_pref.retain(|provider| {
            drive_records.values().all(|record| {
                record
                    .storage
                    .provider_volume()
                    .is_none_or(|volume| volume.provider == *provider)
            })
        });
        if let Some(owner) = native_snapshot_provider {
            provider_pref.retain(|provider| *provider == owner);
        }
        let Some(&preferred_provider) = provider_pref.first() else {
            self.refund_initial_quota(initial_quota_reserved, &tenant, cpu_seconds, ttl_seconds, ctx)
                .await;
            return Err(LeaseResponse::Denied(LeaseError::NoProviderAvailable));
        };

        let mut dispatch_req = req.clone();
        let mut tag_resolved_digest = None;

        if let SandboxImage::SnapshotTag { tag, create_from, .. } = &req.spec.image() {
            match self.tag_store.get(tag).await {
                Err(e) => {
                    self.refund_initial_quota(initial_quota_reserved, &tenant, cpu_seconds, ttl_seconds, ctx)
                        .await;
                    warn!(error = %e, tenant = %tenant, tag = %tag.name, "lease request denied: tag store error");
                    return Err(LeaseResponse::Denied(LeaseError::Backend { message: e.to_string() }));
                }
                Ok(None) => match create_from {
                    Some(fallback_image) => {
                        warn!(tenant = %tenant, tag = %tag.name, "snapshot tag not found: falling back to create_from image");
                        match with_image(&dispatch_req.spec, (**fallback_image).clone()) {
                            Ok(spec) => dispatch_req.spec = spec,
                            Err(e) => return Err(LeaseResponse::Denied(e)),
                        }
                    }
                    None => {
                        self.refund_initial_quota(initial_quota_reserved, &tenant, cpu_seconds, ttl_seconds, ctx)
                            .await;
                        warn!(tenant = %tenant, tag = %tag.name, "lease request denied: snapshot tag not found");
                        return Err(LeaseResponse::Denied(LeaseError::SnapshotTagNotFound {
                            tag: tag.clone(),
                        }));
                    }
                },
                Ok(Some(resolved)) => {
                    tag_resolved_digest = Some(resolved.digest.clone());
                    let repository = self
                        .snapshot_repository_for_restore(&tag.tenant, &resolved.digest, resolved.artifact_type)
                        .await;
                    let image = SandboxImage::Snapshot {
                        reference: OciArtifactRef {
                            registry: self.config.snapshot_registry.clone(),
                            repository,
                            digest: resolved.digest,
                            artifact_type: resolved.artifact_type,
                        },
                        parent: None,
                    };
                    match with_image(&dispatch_req.spec, image) {
                        Ok(spec) => dispatch_req.spec = spec,
                        Err(e) => return Err(LeaseResponse::Denied(e)),
                    }
                }
            }
        }

        let mut capability_request = req.clone();
        capability_request.spec = match with_remote_drive_mounts(&req.spec, drive_records)
            .and_then(|spec| with_bucket_backed_drives(&spec, drive_records))
        {
            Ok(spec) => spec,
            Err(error) => return Err(LeaseResponse::Denied(error)),
        };
        let mut effective_capabilities = infer_capability_requirements(&capability_request);
        let restoring = tag_resolved_digest
            .as_ref()
            .or_else(|| dispatch_req.spec.image().snapshot_digest());
        if let Some(env) = derive_env_digest(self.snapshot_kv.as_ref(), &req.tenant, restoring).await {
            effective_capabilities.get_or_insert_with(Default::default).env_digest = Some(env);
        }
        let required = effective_capabilities.as_ref();
        // A restricting network policy is a security boundary, so it is checked
        // against worker profiles even when the registry is not enforced: an
        // unenforced registry only relaxes "a worker must be heartbeating", not
        // "the worker must be able to honour the egress policy".
        let egress_gate = if self.config.enforce_worker_registry {
            None
        } else {
            required.and_then(egress_requirements_only)
        };
        let snapshot_digest = tag_resolved_digest
            .as_ref()
            .or_else(|| dispatch_req.spec.image().snapshot_digest());

        let snapshot_input = match &dispatch_req.spec.image() {
            SandboxImage::Snapshot { reference, .. } => Some(reference.artifact_type),
            SandboxImage::SnapshotTag { artifact_type, .. } => Some(*artifact_type),
            _ => None,
        };
        let snapshot_output = req.spec.snapshot_out().map(|policy| policy.artifact_type);
        let native_bindings: Vec<_> = req
            .spec
            .drives()
            .iter()
            .filter_map(|mount| {
                drive_records
                    .get(&mount.drive_id)
                    .and_then(|record| record.storage.provider_volume())
                    .map(|storage| trogon_sandbox_contract::NativeVolumeBinding {
                        mount: mount.clone(),
                        storage: storage.clone(),
                    })
            })
            .collect();
        let region_order = region_order(req);
        let targeted_e2b_available = !self.config.enforce_worker_registry
            && provider_pref.first() == Some(&ProviderKind::E2b)
            && self
                .worker_registry
                .eligible_for(ProviderKind::E2b, egress_gate.as_ref())
                .await
                .iter()
                .any(|worker| {
                    worker.supports_lease_epoch.is_supported()
                        && worker.dispatch_protocol == trogon_sandbox_contract::WorkerDispatchProtocol::TargetedV1
                });
        let selection = if self.config.enforce_worker_registry
            || egress_gate.is_some()
            || !region_order.is_empty()
            || !native_bindings.is_empty()
            || snapshot_input.is_some()
            || snapshot_output.is_some()
            || native_snapshot_provider.is_some()
            || crate::worker_registry::requires_targeted_dispatch(preferred_provider)
            || targeted_e2b_available
        {
            match select_provider_in_regions(
                self.worker_registry.as_ref(),
                &provider_pref,
                if self.config.enforce_worker_registry {
                    required
                } else {
                    egress_gate.as_ref()
                },
                snapshot_digest,
                WorkerSelectionConstraints {
                    native_volume_bindings: &native_bindings,
                    snapshot_input,
                    snapshot_output,
                    native_snapshot_input: native_snapshot_provider.is_some(),
                },
                &region_order,
            )
            .await
            {
                Ok((selection, chosen_region)) => {
                    if chosen_region.is_some() {
                        dispatch_req.region = chosen_region;
                        dispatch_req.fallback_regions.clear();
                    }
                    selection
                }
                Err(ProviderSelectionError::NoWorkerInRegion(regions)) => {
                    self.refund_initial_quota(initial_quota_reserved, &tenant, cpu_seconds, ttl_seconds, ctx)
                        .await;
                    warn!(tenant = %tenant, regions = ?regions, "lease request denied: no worker in the requested regions");
                    self.metrics
                        .record_dispatch(tenant.as_str(), ProviderKind::Mock, "denied_no_worker_in_region");
                    return Err(LeaseResponse::Denied(LeaseError::NoWorkerInRegion { regions }));
                }
                Err(ProviderSelectionError::NoWorkerCanRestoreSnapshot(digest)) => {
                    self.refund_initial_quota(initial_quota_reserved, &tenant, cpu_seconds, ttl_seconds, ctx)
                        .await;
                    warn!(
                        tenant = %tenant,
                        digest = %digest,
                        "lease request denied: no worker can restore snapshot"
                    );
                    self.metrics
                        .record_dispatch(tenant.as_str(), ProviderKind::Mock, "denied_no_restore");
                    return Err(LeaseResponse::Denied(LeaseError::NoWorkerCanRestoreSnapshot { digest }));
                }
                Err(ProviderSelectionError::NoWorkerSatisfiesCapabilities(requirements)) => {
                    self.refund_initial_quota(initial_quota_reserved, &tenant, cpu_seconds, ttl_seconds, ctx)
                        .await;
                    warn!(
                        tenant = %tenant,
                        "lease request denied: no worker satisfies required capabilities"
                    );
                    self.metrics
                        .record_dispatch(tenant.as_str(), ProviderKind::Mock, "denied_capabilities");
                    return Err(LeaseResponse::Denied(LeaseError::NoWorkerSatisfiesCapabilities {
                        requirements,
                    }));
                }
                Err(ProviderSelectionError::NoProviderAvailable) => {
                    self.refund_initial_quota(initial_quota_reserved, &tenant, cpu_seconds, ttl_seconds, ctx)
                        .await;
                    warn!(tenant = %tenant, "lease request denied: no worker available for any preferred provider");
                    self.metrics
                        .record_dispatch(tenant.as_str(), ProviderKind::Mock, "denied_no_worker");
                    return Err(LeaseResponse::Denied(LeaseError::NoProviderAvailable));
                }
            }
        } else {
            ProviderSelection {
                provider: preferred_provider,
                locality_hit: false,
                worker_id: None,
            }
        };

        Ok(SelectedLease {
            dispatch_req,
            tag_resolved_digest,
            selection,
        })
    }
}
