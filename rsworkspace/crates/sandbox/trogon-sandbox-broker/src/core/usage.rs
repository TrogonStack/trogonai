use super::*;

impl BrokerCore {
    /// Usage counters for one lease. A live lease reports its running figures
    /// plus resource gauges when its provider declares it can report them. A
    /// released lease returns its final figures (with `ended_at`) until the
    /// usage store ages them out; an unknown or foreign lease is
    /// `LeaseNotFound`, so a caller cannot probe other tenants' ids.
    pub async fn handle_get_lease_usage(&self, req: GetLeaseUsageRequest) -> GetLeaseUsageResponse {
        let now = SystemTime::now();
        let live = match self.active_leases.get(&req.lease_id).await {
            Ok(found) => found,
            Err(e) => {
                warn!(error = %e, "get usage denied: active lease lookup failed");
                return GetLeaseUsageResponse::Denied(LeaseError::Backend { message: e.to_string() });
            }
        };
        let stored = match self.usage.get(&req.tenant, &req.lease_id).await {
            Ok(entry) => entry,
            Err(e) => {
                warn!(error = %e, "get usage denied: usage store lookup failed");
                return GetLeaseUsageResponse::Denied(LeaseError::Backend { message: e.to_string() });
            }
        };
        let not_found = || {
            GetLeaseUsageResponse::Denied(LeaseError::LeaseNotFound {
                lease_id: req.lease_id.clone(),
            })
        };
        match live {
            Some(record) if record.tenant == req.tenant => {
                let entry = stored.unwrap_or_else(|| UsageEntry::new(&record));
                let mut usage = entry.to_usage(record.status, now);
                usage.gauges = self.fetch_gauges(&record).await;
                GetLeaseUsageResponse::Usage { usage }
            }
            Some(_) => not_found(),
            None => match stored {
                Some(entry) if entry.tenant == req.tenant => {
                    let ended = entry.ended_at.unwrap_or(now);
                    GetLeaseUsageResponse::Usage {
                        usage: entry.to_usage(LeaseStatus::Released, ended),
                    }
                }
                _ => not_found(),
            },
        }
    }

    /// Counters summed over the tenant's live leases plus the released leases
    /// whose final figures the usage store still keeps.
    pub async fn handle_get_tenant_usage(&self, req: GetTenantUsageRequest) -> GetTenantUsageResponse {
        let backend =
            |e: &dyn std::fmt::Display| GetTenantUsageResponse::Denied(LeaseError::Backend { message: e.to_string() });
        let records = match self.active_leases.all().await {
            Ok(r) => r,
            Err(e) => return backend(&e),
        };
        let stored = match self.usage.tenant_entries(&req.tenant).await {
            Ok(entries) => entries,
            Err(e) => return backend(&e),
        };
        let mut by_lease: HashMap<String, UsageEntry> = stored
            .into_iter()
            .map(|entry| (entry.lease_id.as_str().to_owned(), entry))
            .collect();
        let now = SystemTime::now();
        let mut total = TenantUsage::default();
        let fold = |total: &mut TenantUsage, usage: &trogon_sandbox_contract::LeaseUsage| {
            total.commands_run = total.commands_run.saturating_add(usage.commands_run);
            total.command_output_bytes = total.command_output_bytes.saturating_add(usage.command_output_bytes);
            total.file_bytes_in = total.file_bytes_in.saturating_add(usage.file_bytes_in);
            total.file_bytes_out = total.file_bytes_out.saturating_add(usage.file_bytes_out);
            total.running_seconds = total.running_seconds.saturating_add(usage.running_seconds);
            total.paused_seconds = total.paused_seconds.saturating_add(usage.paused_seconds);
            total.suspended_seconds = total.suspended_seconds.saturating_add(usage.suspended_seconds);
        };
        for record in records.iter().filter(|r| r.tenant == req.tenant) {
            let entry = by_lease
                .remove(record.lease_id.as_str())
                .unwrap_or_else(|| UsageEntry::new(record));
            total.live_leases += 1;
            fold(&mut total, &entry.to_usage(record.status, now));
        }
        // What is left has no live record: count it only once it has ended, so
        // a lease mid-release is not reported as both live and gone.
        for entry in by_lease.values() {
            let Some(ended) = entry.ended_at else {
                continue;
            };
            total.ended_leases += 1;
            fold(&mut total, &entry.to_usage(LeaseStatus::Released, ended));
        }
        GetTenantUsageResponse::Usage { usage: total }
    }

    /// The tenant's limits next to its consumption. Reads the same sources
    /// admission uses, so a caller sees what the next lease request would.
    pub async fn handle_get_account_limits(&self, req: GetAccountLimitsRequest) -> GetAccountLimitsResponse {
        let backend = |e: &dyn std::fmt::Display| {
            GetAccountLimitsResponse::Denied(LeaseError::Backend { message: e.to_string() })
        };
        let limit = match self.tenant_limits.for_tenant(&req.tenant).await {
            Ok(l) => l,
            Err(e) => return backend(&e),
        };
        let quota = match self.quota.quota_record(&req.tenant).await {
            Ok(q) => q,
            Err(e) => return backend(&e),
        };
        let records = match self.active_leases.all().await {
            Ok(r) => r,
            Err(e) => return backend(&e),
        };
        let live: Vec<&LeaseRecord> = records
            .iter()
            .filter(|r| r.tenant == req.tenant)
            .filter(|r| !matches!(r.status, LeaseStatus::Released | LeaseStatus::Failed))
            .collect();
        let per_tag = limit.as_ref().and_then(|l| l.per_tag_quota.as_ref()).map(|policy| {
            let mut in_use = BTreeMap::new();
            for record in &live {
                if let Some(value) = record.tags.get(&policy.tag_key) {
                    *in_use.entry(value.to_string()).or_insert(0u32) += 1;
                }
            }
            TagLimits {
                tag_key: policy.tag_key.clone(),
                max_active_leases: policy.max_active_leases,
                in_use,
            }
        });
        let (cpu_used, cpu_cap, wall_used, wall_cap, lease_cap) = match &quota {
            Some(q) => (
                q.cpu_seconds_used,
                q.cap_cpu_seconds,
                q.wall_seconds_used,
                q.cap_wall_seconds,
                q.active_lease_cap,
            ),
            None => (
                0,
                self.config.default_cpu_cap,
                0,
                self.config.default_wall_cap,
                self.config.default_active_lease_cap,
            ),
        };
        GetAccountLimitsResponse::Limits {
            limits: AccountLimits {
                active_leases: live.len() as u32,
                max_active_leases: lease_cap,
                cpu_seconds_used: cpu_used,
                cpu_seconds_cap: cpu_cap,
                wall_seconds_used: wall_used,
                wall_seconds_cap: wall_cap,
                max_cpu_seconds_per_request: limit
                    .as_ref()
                    .map(|l| l.max_cpu_seconds_per_request)
                    .unwrap_or(self.config.default_max_cpu_per_request),
                max_memory_mib_per_request: limit
                    .as_ref()
                    .map(|l| l.max_memory_mib_per_request)
                    .unwrap_or(self.config.default_max_memory_mib_per_request),
                per_tag,
            },
        }
    }

    /// Asks the owning worker for resource gauges, only when its profile says
    /// it can report them. Any failure leaves the gauges absent: a missing
    /// figure is honest, a guessed one is not.
    async fn fetch_gauges(&self, record: &LeaseRecord) -> Option<ResourceGauges> {
        let requirements = CapabilityRequirements {
            needs_usage_gauges: CapabilityNeed::Required,
            ..Default::default()
        };
        let profile = self.worker_registry.profile_for_provider(record.provider).await?;
        if !profile.satisfies(&requirements) || record.status != LeaseStatus::Running {
            return None;
        }
        let body = serde_json::to_vec(&ResourceUsageRequest {
            lease_id: record.lease_id.clone(),
        })
        .ok()?;
        let subject = subjects::provider_resource_usage(record.provider, &record.lease_id);
        let reply = match self.bus.request(&subject, Bytes::from(body)).await {
            Ok(r) => r,
            Err(e) => {
                warn!(error = %e, lease_id = %record.lease_id, "resource gauges unavailable: worker dispatch failed");
                return None;
            }
        };
        match serde_json::from_slice::<ResourceUsageResponse>(&reply) {
            Ok(ResourceUsageResponse::Gauges(gauges)) => Some(gauges),
            Ok(ResourceUsageResponse::Unsupported) => None,
            Ok(ResourceUsageResponse::Denied(e)) => {
                warn!(error = ?e, lease_id = %record.lease_id, "resource gauges unavailable: worker denied");
                None
            }
            Err(e) => {
                warn!(error = %e, lease_id = %record.lease_id, "resource gauges unavailable: reply decode failed");
                None
            }
        }
    }
}
