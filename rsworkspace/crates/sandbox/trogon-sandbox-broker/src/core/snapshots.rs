use super::*;
use crate::generated_ulid::generated_ulid;
use trogon_sandbox_contract::CapabilityNeed;
use trogon_sandbox_contract::NativeSnapshotDeleteResponse as Response;

impl BrokerCore {
    pub async fn handle_native_snapshot_delete(
        &self,
        mut req: trogon_sandbox_contract::NativeSnapshotDeleteRequest,
    ) -> trogon_sandbox_contract::NativeSnapshotDeleteResponse {
        if req.reference.provider != ProviderKind::E2b {
            return Response::Unsupported;
        }
        let backend = |error: &dyn std::fmt::Display| {
            Response::Denied(LeaseError::Backend {
                message: format!("native snapshot deletion remains unconfirmed: {error}"),
            })
        };
        req.reference = match self.native_snapshots.begin_delete(&req.tenant, &req.reference).await {
            Ok(Some(reference)) => reference,
            Ok(None) => return Response::NotFound,
            Err(error) => return backend(&error),
        };
        let payload = match serde_json::to_vec(&req) {
            Ok(payload) => Bytes::from(payload),
            Err(error) => return backend(&error),
        };
        let subject = subjects::provider_native_snapshot_delete(req.reference.provider);
        let reply = match self
            .bus
            .request_with_timeout(&subject, payload, self.config.provider_dispatch_timeout)
            .await
        {
            Ok(reply) => reply,
            Err(error) => return backend(&error),
        };
        let response: Response = match serde_json::from_slice(&reply) {
            Ok(response) => response,
            Err(error) => return backend(&error),
        };
        if matches!(response, Response::Deleted | Response::NotFound)
            && let Err(error) = self.native_snapshots.finish_delete(&req.tenant, &req.reference).await
        {
            return backend(&error);
        }
        response
    }

    /// Looks up a single snapshot's [`SnapshotListRow`] by digest.
    ///
    /// Records written before the builder persisted lineage carry no
    /// `created_at`, which is then approximated with `SystemTime::now()`.
    pub async fn handle_snapshot_get(&self, req: SnapshotGetRequest) -> SnapshotGetResponse {
        match self.snapshot_kv.get_owned(&req.tenant, &req.digest).await {
            Ok(Some(record)) if record.tenant == req.tenant => SnapshotGetResponse::Found {
                row: SnapshotListRow {
                    digest: record.digest,
                    tenant: record.tenant,
                    artifact_type: record.artifact_type,
                    created_at: record.created_at.unwrap_or_else(SystemTime::now),
                    parent: record.parent,
                },
            },
            Ok(_) => SnapshotGetResponse::NotFound,
            Err(e) => {
                warn!(error = %e, tenant = %req.tenant, digest = %req.digest.as_str(), "snapshot_get: snapshot kv lookup failed");
                SnapshotGetResponse::NotFound
            }
        }
    }

    /// Captures a snapshot of a running lease without stopping it and returns
    /// a receipt that can seed a new lease. Providers that cannot checkpoint a
    /// running lease reply `Unsupported`.
    pub async fn handle_snapshot_lease(&self, req: SnapshotLeaseRequest) -> SnapshotLeaseResponse {
        let record = match self.owned_lease(&req.tenant, &req.lease_id).await {
            Ok(r) => r,
            Err(e) => return SnapshotLeaseResponse::Denied(e),
        };
        if record.status != LeaseStatus::Running {
            return SnapshotLeaseResponse::Denied(self.not_running_error(&record).await);
        }
        if !record.spec.retention().snapshots.is_allowed() {
            return SnapshotLeaseResponse::Denied(LeaseError::SnapshotsDisabled { lease_id: req.lease_id });
        }
        match self.checkpoint_running_source(&record).await {
            Ok(CheckpointedSource::Oci(receipt)) => SnapshotLeaseResponse::Created {
                receipt: LeaseSnapshotReceipt {
                    reference: OciArtifactRef {
                        registry: self.config.snapshot_registry.clone(),
                        repository: snapshot_repository_for_tenant(&record.tenant),
                        digest: receipt.digest,
                        artifact_type: receipt.artifact_type,
                    },
                    bytes: receipt.bytes,
                },
            },
            // A caller that cannot read a native receipt gets the answer it
            // already understood for a provider that cannot snapshot a
            // running lease.
            Ok(CheckpointedSource::Native(_)) if !req.accept_native.is_accepted() => SnapshotLeaseResponse::Unsupported,
            Ok(CheckpointedSource::Native(receipt)) => SnapshotLeaseResponse::CreatedNative { receipt },
            Err(LeaseError::CheckpointRunningUnsupported { .. }) => SnapshotLeaseResponse::Unsupported,
            Err(e) => SnapshotLeaseResponse::Denied(e),
        }
    }

    /// Deletes a tenant's snapshot from the registry and the live set, and
    /// releases its quota. A repeat or a foreign digest reports `NotFound`.
    pub async fn handle_snapshot_delete(&self, req: SnapshotDeleteRequest) -> SnapshotDeleteResponse {
        let Some(remover) = &self.snapshot_remover else {
            return SnapshotDeleteResponse::Unsupported;
        };
        let (bytes, generation) = match remover.remove(&req.tenant, &req.digest).await {
            Ok(crate::snapshot_tracker::SnapshotRemoval::Removed { bytes, generation }) => (bytes, generation),
            Ok(crate::snapshot_tracker::SnapshotRemoval::NotFound) => {
                return SnapshotDeleteResponse::NotFound;
            }
            Err(e) => {
                warn!(error = %e, digest = %req.digest, "snapshot delete failed");
                return SnapshotDeleteResponse::Denied(LeaseError::Backend { message: e.to_string() });
            }
        };
        let event = SnapshotDeleted {
            digest: req.digest.clone(),
            tenant: req.tenant.clone(),
            bytes,
            generation,
        };
        match serde_json::to_vec(&event) {
            Ok(payload) => {
                if let Err(e) = self.bus.publish(subjects::SNAPSHOT_DELETED, Bytes::from(payload)).await {
                    warn!(error = %e, digest = %req.digest, "snapshot delete: failed to publish snapshot_deleted");
                }
            }
            Err(e) => warn!(error = %e, "snapshot delete: failed to encode snapshot_deleted"),
        }
        SnapshotDeleteResponse::Deleted { bytes }
    }

    /// Walks a snapshot's lineage: the requested snapshot first, then each
    /// recorded parent in turn. The walk stops at a root snapshot, at a parent
    /// the tenant does not own or that has no record, at an already visited
    /// node, and after `MAX_SNAPSHOT_TREE_DEPTH` nodes.
    pub async fn handle_snapshot_tree(&self, req: SnapshotTreeRequest) -> SnapshotTreeResponse {
        let mut nodes = Vec::new();
        let mut seen = std::collections::HashSet::new();
        let mut next = Some(req.digest.clone());
        while let Some(current) = next.take() {
            if nodes.len() >= MAX_SNAPSHOT_TREE_DEPTH || !seen.insert(current.clone()) {
                break;
            }
            match self.snapshot_kv.get_owned(&req.tenant, &current).await {
                Ok(Some(record)) if record.tenant == req.tenant => {
                    next = record.parent.clone();
                    nodes.push(SnapshotTreeNode {
                        digest: record.digest,
                        parent: record.parent,
                        artifact_type: record.artifact_type,
                        created_at: record.created_at.unwrap_or_else(SystemTime::now),
                    });
                }
                Ok(_) => break,
                Err(e) => {
                    warn!(error = %e, tenant = %req.tenant, "snapshot_tree: snapshot kv lookup failed");
                    break;
                }
            }
        }
        SnapshotTreeResponse { denied: None, nodes }
    }

    /// Points a named snapshot tag at a digest, reusing the existing
    /// `tag_store` (does not duplicate storage). Retries on
    /// `RevisionConflict` like `sandbox-admin`'s `cmd_tag_set`, but always
    /// with `expected_revision: None` since this is a client "set" verb, not
    /// an admin CAS-by-revision.
    ///
    /// [`SetSnapshotTagRequest`] has no `artifact_type` field, so
    /// `ArtifactType::SandboxRootfsV1` is hardcoded here until the wire type
    /// carries one.
    pub async fn handle_snapshot_tag_set(&self, req: SetSnapshotTagRequest) -> SetSnapshotTagResponse {
        let tag = SnapshotTag {
            tenant: req.tenant.clone(),
            name: req.name.clone(),
        };

        let record = SnapshotTagRecord {
            digest: req.digest.clone(),
            artifact_type: ArtifactType::SandboxRootfsV1,
            set_by: ServiceAccountId::new("sandbox-broker"),
            set_at: SystemTime::now(),
            retention: None,
        };

        for attempt in 0..SNAPSHOT_TAG_MUTATION_MAX_RETRIES {
            match self.tag_store.set(&tag, record.clone(), None).await {
                Ok(_) => return SetSnapshotTagResponse::Applied,
                Err(SnapshotTagStoreError::RevisionConflict) if attempt + 1 < SNAPSHOT_TAG_MUTATION_MAX_RETRIES => {
                    continue;
                }
                Err(e) => {
                    warn!(error = %e, tenant = %req.tenant, name = %req.name, "snapshot_tag_set denied: tag store error");
                    return SetSnapshotTagResponse::Denied(LeaseError::Backend { message: e.to_string() });
                }
            }
        }

        warn!(tenant = %req.tenant, name = %req.name, "snapshot_tag_set denied: revision conflict after max retries");
        SetSnapshotTagResponse::Denied(LeaseError::Backend {
            message: "tag store CAS failed after max retries".to_string(),
        })
    }

    /// Removes a named snapshot tag, reusing the existing `tag_store`.
    /// Mirrors `sandbox-admin`'s `cmd_tag_clear` retry shape, re-reading the
    /// current revision on each `RevisionConflict` retry.
    pub async fn handle_snapshot_tag_delete(&self, req: DeleteSnapshotTagRequest) -> DeleteSnapshotTagResponse {
        let tag = SnapshotTag {
            tenant: req.tenant.clone(),
            name: req.name.clone(),
        };

        for attempt in 0..SNAPSHOT_TAG_MUTATION_MAX_RETRIES {
            let revision = match self.tag_store.get_entry(&tag).await {
                Ok(Some((_, revision))) => revision,
                Ok(None) => return DeleteSnapshotTagResponse::NotFound,
                Err(e) => {
                    warn!(error = %e, tenant = %req.tenant, name = %req.name, "snapshot_tag_delete denied: tag store lookup failed");
                    return DeleteSnapshotTagResponse::Denied(LeaseError::Backend { message: e.to_string() });
                }
            };

            match self.tag_store.clear(&tag, revision).await {
                Ok(()) => return DeleteSnapshotTagResponse::Applied,
                Err(SnapshotTagStoreError::RevisionConflict) if attempt + 1 < SNAPSHOT_TAG_MUTATION_MAX_RETRIES => {
                    continue;
                }
                Err(SnapshotTagStoreError::NotFound) => return DeleteSnapshotTagResponse::NotFound,
                Err(e) => {
                    warn!(error = %e, tenant = %req.tenant, name = %req.name, "snapshot_tag_delete denied: tag store error");
                    return DeleteSnapshotTagResponse::Denied(LeaseError::Backend { message: e.to_string() });
                }
            }
        }

        warn!(tenant = %req.tenant, name = %req.name, "snapshot_tag_delete denied: revision conflict after max retries");
        DeleteSnapshotTagResponse::Denied(LeaseError::Backend {
            message: "tag store CAS failed after max retries".to_string(),
        })
    }

    /// Lists the history of digests a named snapshot tag has pointed at, via
    /// `tag_history` (needs a live `jetstream::Context`, so it is a separate
    /// reader from `tag_store`, see [`SnapshotTagHistoryReader`]). Reader
    /// errors are warned and return an empty list rather than failing the
    /// whole reply.
    pub async fn handle_snapshot_tag_history(&self, req: SnapshotTagHistoryRequest) -> SnapshotTagHistoryResponse {
        let events = match self.tag_history.read(&req.tenant, &req.name).await {
            Ok(events) => events,
            Err(e) => {
                warn!(error = %e, tenant = %req.tenant, name = %req.name, "snapshot_tag_history: reader failed, returning empty history");
                return SnapshotTagHistoryResponse {
                    denied: None,
                    entries: vec![],
                };
            }
        };

        let entries = events
            .into_iter()
            .filter_map(|event| match event {
                SnapshotTagAuditEvent::Set { next, at, .. } => Some(SnapshotTagAuditEntry {
                    name: req.name.clone(),
                    digest: next,
                    set_at: at,
                }),
                SnapshotTagAuditEvent::Cleared { .. } => None,
            })
            .collect();

        SnapshotTagHistoryResponse { denied: None, entries }
    }

    /// Whether `tenant` may restore `snapshot` through a lease asking for
    /// `provider_pref`. An unknown snapshot and another tenant's read the same,
    /// so the reply does not reveal that it exists.
    pub(super) async fn check_native_snapshot(
        &self,
        tenant: &TenantId,
        snapshot: &trogon_sandbox_contract::NativeSnapshotRef,
        provider_pref: &[ProviderKind],
    ) -> Result<(), LeaseError> {
        let now = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        if snapshot.is_expired_at(now) {
            return Err(LeaseError::InvalidSpec {
                reason: "native snapshot has expired".to_string(),
            });
        }
        if !provider_pref.contains(&snapshot.provider) {
            return Err(LeaseError::InvalidSpec {
                reason: "native snapshot belongs to a provider the request did not list".to_string(),
            });
        }
        match self.native_snapshots.owns(tenant, snapshot).await {
            Ok(true) => Ok(()),
            Ok(false) => Err(LeaseError::InvalidSpec {
                reason: "native snapshot not found".to_string(),
            }),
            Err(e) => Err(LeaseError::Backend {
                message: format!("native snapshot lookup failed: {e}"),
            }),
        }
    }

    /// Live-checkpoints `record`'s running lease via the owning worker, without
    /// stopping it. Routed the same way as cancel: broadcast to every worker
    /// for `record.provider`, and only the worker holding `record.lease_id`
    /// replies.
    pub(super) async fn checkpoint_running_source(
        &self,
        record: &LeaseRecord,
    ) -> Result<CheckpointedSource, LeaseError> {
        let requirements = CapabilityRequirements {
            needs_checkpoint_running: CapabilityNeed::Required,
            ..Default::default()
        };
        if self.config.enforce_worker_registry
            && !self
                .worker_registry
                .any_eligible(record.provider, Some(&requirements))
                .await
        {
            return Err(LeaseError::CheckpointRunningUnsupported {
                provider: record.provider,
            });
        }

        let snapshot_profile = self
            .worker_registry
            .profile_for_provider(record.provider)
            .await
            .map(|profile| profile.snapshot);
        let artifact_type = match record.spec.snapshot_out().map(|p| p.artifact_type) {
            Some(t) => Some(t),
            None => snapshot_profile
                .as_ref()
                .and_then(|snapshot| snapshot.supported_artifact_types.first().copied()),
        };
        // A provider with no OCI format of its own that keeps snapshots as
        // named objects is asked for one of those. The target is then only
        // a placeholder the worker ignores.
        let native_name = match (artifact_type, &snapshot_profile) {
            (None, Some(snapshot)) if snapshot.supports_native_snapshots.is_supported() => {
                Some(format!("trogon-{}", generated_ulid().to_lowercase()))
            }
            _ => None,
        };
        let Some(artifact_type) = artifact_type.or(native_name.as_ref().map(|_| ArtifactType::SandboxRootfsV1)) else {
            return Err(LeaseError::CheckpointRunningUnsupported {
                provider: record.provider,
            });
        };

        let payload = CheckpointRunningRequest {
            lease_id: record.lease_id.clone(),
            target: SnapshotPushTarget {
                registry: self.config.snapshot_registry.clone(),
                repository: snapshot_repository_for_tenant(&record.tenant),
                artifact_type,
            },
            native_name,
        };
        let body =
            Bytes::from(serde_json::to_vec(&payload).map_err(|e| LeaseError::Backend { message: e.to_string() })?);

        let accounting = self.quota.snapshot_store();
        let reservation = if payload.native_name.is_none() {
            let accounting = accounting.as_ref().ok_or_else(|| LeaseError::Backend {
                message: "snapshot accounting store is unavailable".into(),
            })?;
            Some(
                accounting
                    .reserve(
                        &record.tenant,
                        &record.lease_id,
                        &format!("capture:{}", generated_ulid()),
                    )
                    .await
                    .map_err(|reason| LeaseError::Backend {
                        message: format!("snapshot reservation refused: {reason:?}"),
                    })?,
            )
        } else {
            None
        };
        let subject = trogon_sandbox_protocol::subjects::provider_checkpoint_running(record.provider, &record.lease_id);
        let reply = self
            .bus
            .request(&subject, body)
            .await
            .map_err(|e| LeaseError::Backend { message: e.to_string() })?;

        let outcome: SnapshotExportOutcome = serde_json::from_slice(&reply).map_err(|e| LeaseError::Backend {
            message: format!("checkpoint_running reply decode failed: {e}"),
        })?;

        match outcome {
            SnapshotExportOutcome::Ok(receipt) => {
                let accounting = accounting.as_ref().ok_or_else(|| LeaseError::Backend {
                    message: "OCI accounting store is unavailable".into(),
                })?;
                let reservation = reservation.ok_or_else(|| LeaseError::Backend {
                    message: "OCI receipt has no host capture reservation".into(),
                })?;
                let reference = OciArtifactRef {
                    registry: payload.target.registry.clone(),
                    repository: payload.target.repository.clone(),
                    digest: receipt.digest.clone(),
                    artifact_type: receipt.artifact_type,
                };
                let account_error = |reason| LeaseError::Backend {
                    message: format!("snapshot accounting remains unconfirmed: {reason:?}"),
                };
                accounting
                    .bind(&reservation, reference.clone(), &receipt)
                    .await
                    .map_err(account_error)?;
                self.snapshot_kv
                    .record(&SnapshotKvRecord {
                        tenant: record.tenant.clone(),
                        lease_id: record.lease_id.clone(),
                        digest: receipt.digest.clone(),
                        artifact_type: receipt.artifact_type,
                        registry: reference.registry,
                        repository: reference.repository,
                        created_at: Some(reservation.created_at),
                        parent: record.restored_from.clone(),
                        env_digest: None,
                    })
                    .await
                    .map_err(|error| LeaseError::Backend {
                        message: error.to_string(),
                    })?;
                accounting.seal(&reservation).await.map_err(account_error)?;
                let published = accounting.publication(&reservation).await.map_err(account_error)?;
                accounting.enqueue(&published).await.map_err(account_error)?;
                let live = self.quota.snapshot_live_store().ok_or_else(|| LeaseError::Backend {
                    message: "snapshot live projection is unavailable".into(),
                })?;
                let confirmation = crate::snapshot_tracker::account_snapshot_published(
                    live.as_ref(),
                    self.quota.as_ref(),
                    self.quota.snapshot_defaults(),
                    &published,
                )
                .await;
                if !matches!(
                    confirmation,
                    trogon_sandbox_contract::SnapshotAccountingResponse::Committed {
                        disposition: trogon_sandbox_contract::SnapshotAccountingDisposition::Live,
                        ..
                    }
                ) {
                    return Err(LeaseError::Backend {
                        message: format!("snapshot accounting remains pending: {confirmation:?}"),
                    });
                }
                Ok(CheckpointedSource::Oci(receipt))
            }
            SnapshotExportOutcome::Native(receipt) => {
                if receipt.reference.provider != record.provider {
                    return Err(LeaseError::CheckpointRunningFailed {
                        reason: "worker returned a native snapshot of another provider".into(),
                    });
                }
                self.native_snapshots
                    .record(&record.tenant, &receipt.reference)
                    .await
                    .map_err(|e| LeaseError::Backend {
                        message: format!("native snapshot record failed: {e}"),
                    })?;
                Ok(CheckpointedSource::Native(receipt))
            }
            SnapshotExportOutcome::Err(e) => Err(LeaseError::CheckpointRunningFailed {
                reason: format!("{e:?}"),
            }),
        }
    }
}
