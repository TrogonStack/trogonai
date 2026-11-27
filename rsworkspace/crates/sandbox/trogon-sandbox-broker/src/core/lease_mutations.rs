use super::*;

impl BrokerCore {
    /// Forks a currently running lease into a brand-new, independent lease
    /// seeded from a live checkpoint, leaving the source lease running
    /// untouched. Reuses [`Self::handle_lease_request`] to grant the new
    /// lease, so the new lease is subject to the exact same quota,
    /// image-allowlist, and capability checks as any other lease.
    #[tracing::instrument(skip_all, fields(source_lease_id = %req.source_lease_id))]
    pub async fn handle_fork_request(&self, req: LeaseForkRequest) -> LeaseResponse {
        let record = match self.active_leases.get(&req.source_lease_id).await {
            Ok(Some(r)) => r,
            Ok(None) => {
                warn!(lease_id = %req.source_lease_id, "fork denied: source lease not found");
                return LeaseResponse::Denied(LeaseError::SourceLeaseNotFound {
                    lease_id: req.source_lease_id,
                });
            }
            Err(e) => {
                warn!(error = %e, "fork denied: active lease lookup failed");
                return LeaseResponse::Denied(LeaseError::Backend { message: e.to_string() });
            }
        };

        // Don't leak whether a lease exists under a tenant the caller isn't
        // authorized for; report it the same as not-found.
        if record.tenant != req.tenant {
            warn!(lease_id = %req.source_lease_id, "fork denied: source lease belongs to a different tenant");
            return LeaseResponse::Denied(LeaseError::SourceLeaseNotFound {
                lease_id: req.source_lease_id,
            });
        }

        if record.status != LeaseStatus::Running {
            warn!(lease_id = %req.source_lease_id, status = ?record.status, "fork denied: source lease is not running");
            return LeaseResponse::Denied(LeaseError::SourceLeaseNotRunning {
                lease_id: req.source_lease_id,
            });
        }

        if !record.spec.retention().snapshots.is_allowed() {
            warn!(lease_id = %req.source_lease_id, "fork denied: source lease has snapshots off");
            return LeaseResponse::Denied(LeaseError::SnapshotsDisabled {
                lease_id: req.source_lease_id,
            });
        }

        // A retried fork must replay the earlier outcome. Checkpointing first
        // would take (and orphan) a fresh snapshot on every retry.
        let request_key = match LeaseRequestKey::new(req.idempotency_key.clone()) {
            Ok(key) => key,
            Err(error) => {
                return LeaseResponse::Denied(LeaseError::InvalidSpec {
                    reason: error.to_string(),
                });
            }
        };
        match self.idempotency.get_attempt(&req.tenant, &request_key).await {
            Ok(Some(attempt)) => return self.attempt_response(attempt).await,
            Ok(None) => (),
            Err(error) => {
                return LeaseResponse::Denied(LeaseError::Backend {
                    message: format!("dispatch journal lookup failed: {error}"),
                });
            }
        }
        if let Ok(Some(cached)) = self.idempotency.get(&req.tenant, &req.idempotency_key).await
            && !crate::worker_registry::requires_targeted_dispatch(cached.provider)
        {
            return LeaseResponse::Granted(LeaseGranted {
                created: LeaseCreation::Existing,
                ..cached
            });
        }

        let checkpoint = match self.checkpoint_running_source(&record).await {
            Ok(r) => r,
            Err(e) => {
                warn!(lease_id = %req.source_lease_id, error = %e, "fork denied: checkpoint of running source failed");
                return LeaseResponse::Denied(e);
            }
        };

        let image = match checkpoint {
            CheckpointedSource::Oci(receipt) => SandboxImage::Snapshot {
                reference: OciArtifactRef {
                    registry: self.config.snapshot_registry.clone(),
                    repository: snapshot_repository_for_tenant(&record.tenant),
                    digest: receipt.digest,
                    artifact_type: receipt.artifact_type,
                },
                parent: None,
            },
            CheckpointedSource::Native(receipt) => SandboxImage::NativeSnapshot {
                reference: receipt.reference,
            },
        };
        let spec = match with_image(&record.spec, image) {
            Ok(spec) => spec,
            Err(e) => return LeaseResponse::Denied(e),
        };

        let new_req = LeaseRequest {
            operation_id: None,
            tenant: record.tenant.clone(),
            provider_pref: vec![record.provider],
            spec,
            ttl_seconds: req.ttl_seconds,
            idempotency_key: request_key,
            isolation_tier: None,
            host_imports: Vec::new(),
            required_capabilities: None,
            prewarm: LeasePrewarm::Regular,
            tags: SandboxTags::default(),
            name: None,
            persistent: LeasePersistence::Persistent,
            native: None,
            region: None,
            fallback_regions: Vec::new(),
        };

        self.handle_lease_request(new_req).await
    }

    /// Replaces the `network_policy` of an already-running lease in place. The
    /// updated record is persisted to both the active-lease store (so
    /// subsequent lookups, e.g. by fork or cancel, see the new policy) and the
    /// legacy KV, matching `handle_cancel`'s persistence pattern, and then
    /// dispatched to the owning worker so the sandbox's live egress proxy
    /// actually picks up the new policy (see `dispatch_network_policy_update`).
    pub async fn handle_network_policy_update(&self, req: NetworkPolicyUpdateRequest) -> NetworkPolicyUpdateResponse {
        if let Err(error) = validate_network_policy(&req.network_policy) {
            let reason = error.to_string();
            warn!(reason = %reason, "network policy update denied: invalid network policy");
            return NetworkPolicyUpdateResponse::Denied(LeaseError::InvalidNetworkPolicy { reason });
        }

        let record = match self.active_leases.get(&req.lease_id).await {
            Ok(Some(r)) => r,
            Ok(None) => {
                warn!(lease_id = %req.lease_id, "network policy update denied: lease not found");
                return NetworkPolicyUpdateResponse::Denied(LeaseError::LeaseNotFound { lease_id: req.lease_id });
            }
            Err(e) => {
                warn!(error = %e, "network policy update denied: active lease lookup failed");
                return NetworkPolicyUpdateResponse::Denied(LeaseError::Backend { message: e.to_string() });
            }
        };

        if record.status != LeaseStatus::Running {
            warn!(lease_id = %req.lease_id, status = ?record.status, "network policy update denied: lease is not running");
            return NetworkPolicyUpdateResponse::Denied(LeaseError::LeaseNotRunning { lease_id: req.lease_id });
        }

        let requirements = CapabilityRequirements {
            needs_network_policy_update: CapabilityNeed::Required,
            ..Default::default()
        };
        if !self.provider_satisfies(record.provider, &requirements).await {
            warn!(lease_id = %req.lease_id, provider = ?record.provider, "network policy update denied: provider cannot update a running policy");
            return NetworkPolicyUpdateResponse::Denied(LeaseError::NoWorkerSatisfiesCapabilities { requirements });
        }

        let mut updated = record.clone();
        updated.spec = match with_network_policy(&record.spec, req.network_policy) {
            Ok(spec) => spec,
            Err(e) => return NetworkPolicyUpdateResponse::Denied(e),
        };

        // Committed before the record and the worker see the new policy, so a
        // proxy refreshed by the dispatch finds the revision it is stamped with.
        if let Err(e) = self
            .commit_credential_policy(&updated.tenant, &updated.lease_id, &updated.spec)
            .await
        {
            warn!(error = ?e, "network policy update denied: credential policy commit failed");
            return NetworkPolicyUpdateResponse::Denied(e);
        }

        if let Err(e) = self.update_record(&record, &updated).await {
            warn!(error = %e, "network policy update denied: active lease store write failed");
            return NetworkPolicyUpdateResponse::Denied(LeaseError::Backend { message: e.to_string() });
        }

        let response = self.dispatch_network_policy_update(&updated).await;
        match &response {
            NetworkPolicyUpdateResponse::Applied => {
                info!(lease_id = %req.lease_id, "network policy updated");
            }
            NetworkPolicyUpdateResponse::Denied(e) => {
                warn!(lease_id = %req.lease_id, error = ?e, "network policy update denied by worker");
            }
        }
        response
    }

    /// Rejects a new lease when the tenant already holds `policy.max_active_leases`
    /// live leases carrying the same value for `policy.tag_key` as `tags`.
    ///
    /// A request without that tag key is not limited here. The count is read
    /// from the active lease store, which is not the quota CAS record, so two
    /// concurrent requests for the same value can both pass and overshoot the
    /// cap by the number of racing requests. The tenant-wide active lease cap
    /// stays exact; this per-tag cap is best effort. Closing the race needs a
    /// per-value counter inside the quota record, left for a follow-up.
    pub(super) async fn check_per_tag_quota(
        &self,
        tenant: &TenantId,
        policy: &crate::limits::PerTagQuota,
        tags: &SandboxTags,
    ) -> Result<(), LeaseError> {
        let Some(value) = tags.get(&policy.tag_key) else {
            return Ok(());
        };
        let records = self
            .active_leases
            .all()
            .await
            .map_err(|e| LeaseError::Backend { message: e.to_string() })?;
        let live = records
            .iter()
            .filter(|record| &record.tenant == tenant)
            .filter(|record| !matches!(record.status, LeaseStatus::Released | LeaseStatus::Failed))
            .filter(|record| record.tags.get(&policy.tag_key) == Some(value))
            .count();
        if live >= policy.max_active_leases as usize {
            return Err(LeaseError::TooManyTaggedLeases {
                tenant: tenant.clone(),
                tag_key: policy.tag_key.clone(),
                tag_value: value.to_string(),
                cap: policy.max_active_leases,
            });
        }
        Ok(())
    }

    /// Replaces the full `tags` set of an already-running lease in place. Tags
    /// are pure metadata, so unlike `handle_network_policy_update` this never
    /// dispatches to the owning worker.
    pub async fn handle_update_lease_tags(&self, req: UpdateLeaseTagsRequest) -> UpdateLeaseTagsResponse {
        let record = match self.active_leases.get(&req.lease_id).await {
            Ok(Some(r)) => r,
            Ok(None) => {
                warn!(lease_id = %req.lease_id, "tags update denied: lease not found");
                return UpdateLeaseTagsResponse::Denied(LeaseError::LeaseNotFound { lease_id: req.lease_id });
            }
            Err(e) => {
                warn!(error = %e, "tags update denied: active lease lookup failed");
                return UpdateLeaseTagsResponse::Denied(LeaseError::Backend { message: e.to_string() });
            }
        };

        if record.status != LeaseStatus::Running {
            warn!(lease_id = %req.lease_id, status = ?record.status, "tags update denied: lease is not running");
            return UpdateLeaseTagsResponse::Denied(LeaseError::LeaseNotRunning { lease_id: req.lease_id });
        }

        let mut updated = record.clone();
        updated.tags = req.tags;

        if let Err(e) = self.update_record(&record, &updated).await {
            warn!(error = %e, "tags update denied: active lease store write failed");
            return UpdateLeaseTagsResponse::Denied(LeaseError::Backend { message: e.to_string() });
        }

        info!(lease_id = %req.lease_id, "tags updated");
        UpdateLeaseTagsResponse::Applied
    }

    /// Extends the wall-clock deadline of an already-running lease without
    /// restarting it. Broker-only bookkeeping: no worker dispatch needed.
    pub async fn handle_extend_lease_timeout(&self, req: ExtendLeaseTimeoutRequest) -> ExtendLeaseTimeoutResponse {
        let record = match self.active_leases.get(&req.lease_id).await {
            Ok(Some(r)) => r,
            Ok(None) => {
                warn!(lease_id = %req.lease_id, "extend timeout denied: lease not found");
                return ExtendLeaseTimeoutResponse::Denied(LeaseError::LeaseNotFound { lease_id: req.lease_id });
            }
            Err(e) => {
                warn!(error = %e, "extend timeout denied: active lease lookup failed");
                return ExtendLeaseTimeoutResponse::Denied(LeaseError::Backend { message: e.to_string() });
            }
        };

        if record.status != LeaseStatus::Running {
            warn!(lease_id = %req.lease_id, status = ?record.status, "extend timeout denied: lease is not running");
            return ExtendLeaseTimeoutResponse::Denied(LeaseError::LeaseNotRunning { lease_id: req.lease_id });
        }

        if record.spec.no_ttl().is_unbounded() {
            return ExtendLeaseTimeoutResponse::Extended {
                new_deadline: record.deadline,
            };
        }

        let new_deadline = match validate_lease_extension_with_max(
            record.created_at,
            record.deadline,
            req.additional_seconds,
            self.lifetime.max_lease_lifetime_seconds,
        ) {
            Ok(d) => d,
            Err(e) => return ExtendLeaseTimeoutResponse::Denied(e),
        };

        let mut updated = record.clone();
        updated.deadline = new_deadline;

        if let Err(e) = self.update_record(&record, &updated).await {
            warn!(error = %e, "extend timeout denied: active lease store write failed");
            return ExtendLeaseTimeoutResponse::Denied(LeaseError::Backend { message: e.to_string() });
        }

        info!(lease_id = %req.lease_id, "lease timeout extended");
        ExtendLeaseTimeoutResponse::Extended { new_deadline }
    }

    /// Applies `record`'s (already-persisted) `network_policy` to the sandbox's
    /// live egress proxy via the owning worker. Routed the same way as
    /// checkpoint_running: broadcast to every worker for `record.provider`, and
    /// only the worker holding `record.lease_id` replies.
    async fn dispatch_network_policy_update(&self, record: &LeaseRecord) -> NetworkPolicyUpdateResponse {
        let Some(network_policy) = record.spec.network_policy().cloned() else {
            return NetworkPolicyUpdateResponse::Denied(LeaseError::Backend {
                message: "record has no network_policy to dispatch".to_string(),
            });
        };

        let payload = NetworkPolicyUpdateRequest {
            lease_id: record.lease_id.clone(),
            network_policy,
        };
        let body = match serde_json::to_vec(&payload) {
            Ok(b) => Bytes::from(b),
            Err(e) => {
                return NetworkPolicyUpdateResponse::Denied(LeaseError::Backend { message: e.to_string() });
            }
        };

        let subject =
            trogon_sandbox_protocol::subjects::provider_network_policy_update(record.provider, &record.lease_id);
        let reply = match self.bus.request(&subject, body).await {
            Ok(r) => r,
            Err(e) => {
                return NetworkPolicyUpdateResponse::Denied(LeaseError::Backend { message: e.to_string() });
            }
        };

        match serde_json::from_slice::<NetworkPolicyUpdateResponse>(&reply) {
            Ok(resp) => resp,
            Err(e) => NetworkPolicyUpdateResponse::Denied(LeaseError::Backend {
                message: format!("network_policy_update reply decode failed: {e}"),
            }),
        }
    }
}
