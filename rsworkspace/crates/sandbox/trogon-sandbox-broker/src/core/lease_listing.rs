use super::*;

impl BrokerCore {
    /// Lists leases owned by `req.tenant`, paginated by an opaque cursor.
    ///
    /// Tenant filtering happens before any cursor/limit logic so a record
    /// belonging to a different tenant can never appear in the response or
    /// factor into pagination math.
    pub async fn handle_list_leases(&self, req: ListLeasesRequest) -> ListLeasesResponse {
        let all_records = match self.active_leases.all().await {
            Ok(records) => records,
            Err(e) => {
                warn!(error = %e, tenant = %req.tenant, "list leases denied: active lease store error");
                return ListLeasesResponse {
                    denied: Some(LeaseError::Backend { message: e.to_string() }),
                    leases: Vec::new(),
                    next_cursor: None,
                };
            }
        };

        let mut records: Vec<LeaseRecord> = all_records
            .into_iter()
            .filter(|record| record.tenant == req.tenant)
            .filter(|record| match &req.status_filter {
                Some(status) => record.status == *status,
                None => true,
            })
            .filter(|record| tags_contain_all(&record.tags, &req.tags))
            .collect();
        records.sort_by(|a, b| {
            a.created_at
                .cmp(&b.created_at)
                .then_with(|| a.lease_id.as_str().cmp(b.lease_id.as_str()))
        });

        let start = match &req.cursor {
            Some(cursor) => records
                .iter()
                .position(|record| record.lease_id.as_str() == cursor)
                .map(|idx| idx + 1)
                .unwrap_or(0),
            None => 0,
        };

        let limit = req
            .limit
            .unwrap_or(crate::constants::DEFAULT_LIST_LEASES_LIMIT)
            .min(crate::constants::MAX_LIST_LEASES_LIMIT) as usize;

        let remaining = &records[start.min(records.len())..];
        let page: Vec<&LeaseRecord> = remaining.iter().take(limit).collect();
        let next_cursor = if page.len() < remaining.len() {
            page.last().map(|record| record.lease_id.as_str().to_owned())
        } else {
            None
        };

        let leases = page.into_iter().map(lease_summary).collect();

        ListLeasesResponse {
            denied: None,
            leases,
            next_cursor,
        }
    }

    /// Releases the tenant's leases that match every filter in `req`, or only
    /// reports them on a dry run. Tenant filtering comes first, so one tenant
    /// can never reap another's leases. A lease already released by someone
    /// else (the sweeper, a delete, a concurrent reap) is skipped rather than
    /// reported as a failure, which keeps a repeated call a no-op.
    pub async fn handle_reap_leases(&self, req: ReapLeasesRequest) -> ReapLeasesResponse {
        if req.tags().is_empty() && req.older_than_seconds().is_none() && req.idle_for_seconds().is_none() {
            return ReapLeasesResponse::Denied(LeaseError::InvalidSpec {
                reason: "reap requires at least one of tags, older_than_seconds, idle_for_seconds".to_string(),
            });
        }
        let all_records = match self.active_leases.all().await {
            Ok(records) => records,
            Err(e) => {
                warn!(error = %e, tenant = %req.tenant(), "reap leases denied: active lease store error");
                return ReapLeasesResponse::Denied(LeaseError::Backend { message: e.to_string() });
            }
        };

        let now = SystemTime::now();
        let mut matched: Vec<LeaseRecord> = all_records
            .into_iter()
            .filter(|record| record.tenant == *req.tenant())
            .filter(|record| record.status != LeaseStatus::Released)
            .filter(|record| tags_contain_all(&record.tags, req.tags()))
            .filter(|record| {
                req.older_than_seconds()
                    .is_none_or(|secs| elapsed_at_least(record.created_at, now, secs))
            })
            .filter(|record| {
                req.idle_for_seconds()
                    .is_none_or(|secs| elapsed_at_least(crate::idle::last_activity(record), now, secs))
            })
            .collect();
        matched.sort_by(|a, b| {
            a.created_at
                .cmp(&b.created_at)
                .then_with(|| a.lease_id.as_str().cmp(b.lease_id.as_str()))
        });

        let summaries: Vec<LeaseSummary> = matched.iter().map(lease_summary).collect();
        let mut released = Vec::new();
        let mut failed = Vec::new();
        if !req.dry_run().is_dry_run() {
            for record in &matched {
                match self.active_leases.get(&record.lease_id).await {
                    Ok(None) => continue,
                    Ok(Some(_)) => {}
                    Err(e) => {
                        failed.push(ReapFailure {
                            lease_id: record.lease_id.clone(),
                            message: e.to_string(),
                        });
                        continue;
                    }
                }
                match self.cancel_lease(record.lease_id.clone(), CancelIntent::Release).await {
                    Ok(()) => released.push(record.lease_id.clone()),
                    Err(e) => {
                        warn!(error = %e, lease_id = %record.lease_id, "reap: release failed");
                        failed.push(ReapFailure {
                            lease_id: record.lease_id.clone(),
                            message: e.to_string(),
                        });
                    }
                }
            }
            info!(tenant = %req.tenant(), released = released.len(), failed = failed.len(), "leases reaped");
        }

        ReapLeasesResponse::Reaped {
            dry_run: req.dry_run(),
            matched: summaries,
            released,
            failed,
        }
    }

    /// Looks up a running lease by its tenant-scoped `name` via the
    /// `named_leases` index, which is already populated at lease-grant time.
    /// A stale index entry (pointing at a lease that no longer exists in
    /// `active_leases`) is treated as `NotFound` rather than surfaced as an
    /// error.
    pub async fn handle_get_lease_by_name(&self, req: GetLeaseByNameRequest) -> GetLeaseByNameResponse {
        let lease_id = match self.resolve_named_lease(&req.tenant, req.name.as_str()).await {
            Ok(Some(id)) => id,
            Ok(None) => return GetLeaseByNameResponse::NotFound,
            // The entity is the source of truth in `on` mode: its failure must
            // not read as a free name.
            Err(e @ LeaseError::EntityUnavailable { .. }) => {
                return GetLeaseByNameResponse::Denied(e);
            }
            Err(e) => {
                warn!(error = %e, tenant = %req.tenant, name = %req.name, "get_lease_by_name: named lease index lookup failed");
                return GetLeaseByNameResponse::NotFound;
            }
        };

        let record = match self.active_leases.get(&lease_id).await {
            Ok(Some(r)) => r,
            Ok(None) => {
                warn!(lease_id = %lease_id, tenant = %req.tenant, name = %req.name, "get_lease_by_name: stale named lease index entry");
                return GetLeaseByNameResponse::NotFound;
            }
            Err(e) => {
                warn!(error = %e, lease_id = %lease_id, tenant = %req.tenant, name = %req.name, "get_lease_by_name: active lease lookup failed");
                return GetLeaseByNameResponse::NotFound;
            }
        };

        if record.tenant != req.tenant || record.lease_id != lease_id {
            return GetLeaseByNameResponse::NotFound;
        }
        // A released lease, or one a delete is purging, no longer answers to
        // the name; the name is free for the next lease that takes it.
        if record.status == LeaseStatus::Released || self.purge_pending(&lease_id).await {
            return GetLeaseByNameResponse::NotFound;
        }

        GetLeaseByNameResponse::Found {
            row: LeaseListRow {
                lease_id: record.lease_id.clone(),
                tenant: record.tenant.clone(),
                provider: record.provider,
                status: record.status,
                created_at: record.created_at,
                deadline: record.deadline,
                name: Some(req.name.clone()),
                tags: record.tags.clone(),
            },
        }
    }
}

/// Whether `tags` holds every entry of `filter`.
fn tags_contain_all(tags: &SandboxTags, filter: &SandboxTags) -> bool {
    filter.iter().all(|(key, value)| tags.get(key) == Some(value))
}
