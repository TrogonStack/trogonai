use super::*;

impl BrokerCore {
    pub(super) async fn check_lease_policy(
        &self,
        req: &LeaseRequest,
        ctx: &CreateCtx,
    ) -> Result<Option<ProviderKind>, LeaseResponse> {
        let tenant = req.tenant.clone();
        let is_prewarm = req.prewarm.is_prewarm();
        let cpu_seconds = req.spec.cpu_seconds().get();
        if let Some(native) = &req.native
            && let Err(denied) = self.check_native_request(&tenant, native, req).await
        {
            self.metrics
                .record_dispatch(tenant.as_str(), ProviderKind::Mock, "denied_native");
            return Err(LeaseResponse::Denied(denied));
        }
        if let Err(reason) = validate_regions(req) {
            warn!(tenant = %tenant, reason = %reason, "lease request denied: invalid region");
            return Err(LeaseResponse::Denied(LeaseError::InvalidSpec {
                reason: reason.to_string(),
            }));
        }

        let native_snapshot_provider = match &req.spec.image() {
            SandboxImage::NativeSnapshot { reference } => {
                if let Err(denied) = self.check_native_snapshot(&tenant, reference, &req.provider_pref).await {
                    warn!(tenant = %tenant, "lease request denied: native snapshot not restorable");
                    return Err(LeaseResponse::Denied(denied));
                }
                Some(reference.provider)
            }
            _ => None,
        };

        // A native snapshot is authorized by the tenant's own record of it, so
        // the OCI image allow-list has nothing to match against.
        if !is_prewarm && self.config.enforce_image_allowlist && native_snapshot_provider.is_none() {
            match self.image_allowlist.allowed_for(&tenant).await {
                Err(e) => {
                    warn!(error = %e, "lease request denied: image allow-list lookup failed");
                    return Err(LeaseResponse::Denied(LeaseError::Backend { message: e.to_string() }));
                }
                Ok(None) => {
                    warn!(
                        tenant = %tenant,
                        image = %req.spec.image().display_label(),
                        "lease request denied: no image policy for tenant"
                    );
                    return Err(LeaseResponse::Denied(LeaseError::InvalidSpec {
                        reason: format!(
                            "image '{}' not in allow-list for tenant '{}'",
                            req.spec.image().display_label(),
                            tenant.as_str()
                        ),
                    }));
                }
                Ok(Some(policy)) => {
                    let allowed = req.spec.image().primary_oci_image_ref().map_or_else(
                        || {
                            policy.patterns.iter().any(|p| {
                                req.spec
                                    .image()
                                    .allowlist_match_strings()
                                    .iter()
                                    .any(|candidate| crate::image_policy::pattern_matches(p, candidate))
                            })
                        },
                        |ref_| policy.matches(ref_),
                    );
                    if !allowed {
                        warn!(
                            tenant = %tenant,
                            image = %req.spec.image().display_label(),
                            "lease request denied: image not in allow-list"
                        );
                        return Err(LeaseResponse::Denied(LeaseError::InvalidSpec {
                            reason: format!(
                                "image '{}' not in allow-list for tenant '{}'",
                                req.spec.image().display_label(),
                                tenant.as_str()
                            ),
                        }));
                    }
                }
            }
        }

        if let Some(image_ref) = req.spec.image().primary_oci_image_ref() {
            match self.image_readiness.check(image_ref).await {
                Ok(ImageReadiness::Ready) => {}
                Ok(ImageReadiness::Preparing { reason }) | Ok(ImageReadiness::Incompatible { reason }) => {
                    warn!(
                        tenant = %tenant,
                        image = %req.spec.image().display_label(),
                        reason = %reason,
                        "lease request denied: image not ready"
                    );
                    return Err(LeaseResponse::Denied(LeaseError::ImageNotReady {
                        image: req.spec.image().display_label(),
                        reason,
                    }));
                }
                Err(e) => {
                    warn!(
                        error = %e,
                        image = %req.spec.image().display_label(),
                        "image readiness check failed, treating as ready (fail-open)"
                    );
                }
            }
        }

        if let WorkspaceSource::HostMount { path, .. } = &req.spec.workspace() {
            match self.host_mount_allowlist.allowed_for(&tenant).await {
                Err(e) => {
                    warn!(error = %e, "lease request denied: host_mount allow-list lookup failed");
                    return Err(LeaseResponse::Denied(LeaseError::Backend { message: e.to_string() }));
                }
                Ok(None) => {
                    warn!(tenant = %tenant, "lease request denied: no host_mount policy for tenant");
                    return Err(LeaseResponse::Denied(LeaseError::InvalidSpec {
                        reason: format!("host_mount workspace not permitted for tenant '{}'", tenant.as_str()),
                    }));
                }
                Ok(Some(policy)) => {
                    let allowed = policy
                        .path_prefixes
                        .iter()
                        .any(|prefix| path_prefix_matches(prefix, path));
                    if !allowed {
                        warn!(tenant = %tenant, path = %path, "lease request denied: host_mount path not under allowed prefix");
                        return Err(LeaseResponse::Denied(LeaseError::InvalidSpec {
                            reason: format!(
                                "host_mount path '{path}' not under any allowed prefix for tenant '{}'",
                                tenant.as_str()
                            ),
                        }));
                    }
                }
            }
        }

        let (cpu_cap, mem_cap, per_tag_quota) = match self.tenant_limits.for_tenant(&tenant).await {
            Err(e) => {
                warn!(error = %e, "lease request denied: tenant limits lookup failed");
                return Err(LeaseResponse::Denied(LeaseError::Backend { message: e.to_string() }));
            }
            Ok(Some(limit)) => (
                limit.max_cpu_seconds_per_request,
                limit.max_memory_mib_per_request,
                limit.per_tag_quota,
            ),
            Ok(None) => (
                self.config.default_max_cpu_per_request,
                self.config.default_max_memory_mib_per_request,
                None,
            ),
        };

        if cpu_seconds > cpu_cap {
            warn!(tenant = %tenant, cpu_seconds, cap = cpu_cap, "lease request denied: cpu_seconds exceeds per-request cap");
            return Err(LeaseResponse::Denied(LeaseError::InvalidSpec {
                reason: format!("cpu_seconds {} exceeds per-request cap {}", cpu_seconds, cpu_cap),
            }));
        }

        if req.spec.memory_mib().get() > mem_cap {
            warn!(tenant = %tenant, memory_mib = req.spec.memory_mib().get(), cap = mem_cap, "lease request denied: memory_mib exceeds per-request cap");
            return Err(LeaseResponse::Denied(LeaseError::InvalidSpec {
                reason: format!(
                    "memory_mib {} exceeds per-request cap {}",
                    req.spec.memory_mib().get(),
                    mem_cap
                ),
            }));
        }

        if req.spec.vcpu_millis() == Some(0) || req.spec.disk_mib() == Some(0) {
            warn!(tenant = %tenant, "lease request denied: zero vcpu_millis or disk_mib");
            return Err(LeaseResponse::Denied(LeaseError::InvalidSpec {
                reason: "vcpu_millis and disk_mib must be greater than zero when set".to_string(),
            }));
        }

        if !is_prewarm {
            if let Some(policy) = &per_tag_quota
                && let Err(e) = self.check_per_tag_quota(&tenant, policy, &req.tags).await
            {
                warn!(tenant = %tenant, tag_key = %policy.tag_key, "lease request denied: per-tag lease cap exceeded");
                return Err(LeaseResponse::Denied(e));
            }

            if ctx.operation.is_none()
                && let Err(e) = self.quota.check_active_lease_cap(&tenant).await
            {
                return Err(LeaseResponse::Denied(e));
            }
        } else if ctx.operation.is_none()
            && let Err(e) = self.quota.check_prewarm_dispatch(&tenant, cpu_seconds).await
        {
            return Err(LeaseResponse::Denied(e));
        }
        Ok(native_snapshot_provider)
    }
}
